//! File analysis CLI: plan inputs, reuse AssumeRole, run a cancellable worker, publish one bounded result.
use super::{data_command::DataCommand, data_plan::plan, data_render::print_result};
use crate::{
    adapters::{
        aws::s3_data,
        config::Config,
        data_inputs,
        duckdb::{self, Analysis, S3Access, connection::Cancellation},
    },
    domain::{
        functions::parquet_advice,
        types::{
            dataset::{
                DataError, DataKind, DataMeta, DataOperation, DataOutput, DataPayload,
                InvalidInput, NextAction, RegionSource,
            },
            limits::INPUT_LIST,
        },
    },
    ports::AwsProfileCredentials,
    shell::aws_profile_credentials::AssumeRoleCredentials,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::signal::unix::{SignalKind, signal};

pub async fn run(command: DataCommand, config: Config) -> anyhow::Result<()> {
    let (mut request, url_region) = command.read_request()?;
    if let Some(sql) = &request.args().sql {
        crate::shell::audit::note_sql(sql);
    }
    let mut plan = plan(&command, &request, url_region, &config)?;
    request.select_table(&plan.sources)?;
    let target = command.workspace.clone().unwrap_or_else(|| "ad-hoc".into());
    let operation = request.operation();
    let start = Instant::now();
    // One listener for the whole command, registered before anything starts:
    // a SIGINT reaches the listeners that exist when it arrives, and
    // `ctrl_c()` registers only when first polled, so one that arrived after
    // the worker had started and before the wait polled its listener was
    // lost, and the scan ran on until it was killed.
    let mut interrupt = signal(SignalKind::interrupt())
        .map_err(|_| DataError::Engine("installing the interrupt listener"))?;
    let auth = AssumeRoleCredentials::new(Arc::new(config));
    // Filled in by the worker once the statement is parsed, so a wait that
    // ends early can still name the columns it was reading.
    let scanned = Arc::new(duckdb::ScannedColumns::default());
    let needs_s3 = plan.sources.iter().any(|s| s.path.starts_with("s3://"));
    // A named region is read as it is. Otherwise the profile's region only says
    // where the first call is signed, and the bucket corrects it.
    let named_region = plan.region.is_some();
    let profile = if needs_s3 {
        let name = plan
            .aws_profile
            .as_ref()
            .ok_or(DataError::Invalid(InvalidInput::S3CredentialsRequired))?;
        let profile = auth.load_profile(name).await?;
        plan.region = plan
            .region
            .or_else(|| profile.region_raw().map(str::to_owned));
        if plan.region.is_none() {
            return Err(DataError::Invalid(InvalidInput::S3RegionRequired).into());
        }
        Some(profile)
    } else {
        None
    };
    let mut output = DataOutput {
        schema_version: 1,
        kind: DataKind::Data,
        operation,
        target,
        meta: DataMeta::planned(
            plan.sources.clone(),
            plan.limits.clone(),
            plan.aws_profile.clone(),
            plan.region.clone(),
        ),
        dry_run: None,
        payload: None,
        elapsed_ms: None,
    };
    if plan.region.is_some() {
        output.meta.region_source = Some(if named_region {
            RegionSource::Named
        } else {
            RegionSource::Profile
        });
    }
    if command.dry_run {
        output.dry_run = Some(true);
        if let Some(filter) = &command.jq {
            println!("{}", project_output(&output, filter)?);
        } else {
            print_result(&output, command.json);
        }
        return Ok(());
    }
    let credentials = if let Some(profile) = profile {
        let credentials = tokio::select! {
            result = auth.assume_role(&profile) => result?,
            _ = interrupt.recv() => return Err(DataError::Interrupted(scanned.context(start)).into()),
        };
        if credentials.is_expired(chrono::Utc::now()) {
            return Err(DataError::S3Rejected("ExpiredToken".into()).into());
        }
        Some(credentials)
    } else {
        None
    };
    let mut clients = match (&credentials, &plan.region) {
        (Some(credentials), Some(region)) => Some(s3_data::BucketClients::new(
            credentials,
            region,
            !named_region,
            plan.limits.query_timeout_secs,
        )),
        _ => None,
    };
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(plan.limits.query_timeout_secs);
    let setup = async {
        let mut files = vec![];
        let mut examined = 0;
        for source in &plan.sources {
            let found = if source.path.starts_with("s3://") {
                let clients = clients.as_mut().expect("an S3 input has credentials");
                s3_data::resolve(clients, source, &plan.limits, &mut examined).await?
            } else {
                data_inputs::resolve_local(source, &plan.limits, &mut examined)?
            };
            files.extend(found);
            data_inputs::validate_inputs(&files, &plan.sources, &plan.limits, &request)?;
        }
        Ok::<_, anyhow::Error>(files)
    };
    let files = tokio::select! {
        result = tokio::time::timeout_at(deadline, setup) => result.map_err(|_| DataError::Timeout(scanned.context(start)))??,
        _ = interrupt.recv() => return Err(DataError::Interrupted(scanned.context(start)).into()),
    };
    let s3 = match (credentials, &clients) {
        (Some(credentials), Some(clients)) => {
            output.meta.region = read_regions(clients.regions());
            if !named_region {
                output.meta.region_source = Some(RegionSource::Bucket);
            }
            Some(S3Access {
                credentials,
                regions: clients.regions().clone(),
                #[cfg(feature = "test-fakes")]
                endpoint: std::env::var("KURAMA_TEST_S3_ENDPOINT")
                    .ok()
                    .map(|s| s.trim_start_matches("http://").to_owned()),
            })
        }
        _ => None,
    };
    output.meta.input_count = Some(files.len());
    output.meta.input_bytes = Some(files.iter().map(|f| f.size).sum());
    let (listed, omitted) = INPUT_LIST.split(&files);
    output.meta.inputs = Some(listed.to_vec());
    output.meta.inputs_omitted = Some(omitted);
    output.meta.observed_at = Some(chrono::Utc::now().to_rfc3339());
    let cancel = Arc::new(Cancellation::default());
    let worker_cancel = cancel.clone();
    let analysis = Analysis {
        sources: plan.sources,
        files: files.clone(),
        limits: plan.limits,
        request,
        s3,
    };
    let worker_scanned = scanned.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        duckdb::execute(analysis, worker_cancel, &worker_scanned)
    });
    // A person watching a long scan gets the elapsed seconds and nothing else:
    // how far along it is cannot be observed, so no number claims to know.
    // JSON mode is already silent; a pipe gets no carriage returns either.
    let ticking = !command.json && crate::shell::tui::terminal::rewrites_stderr_lines();
    let scan = Instant::now();
    let mut line = crate::console::ProgressLine::new();
    let mut second = tokio::time::interval(Duration::from_secs(1));
    second.tick().await;
    let mut completed = loop {
        tokio::select! {
            result = &mut worker => break result.map_err(|_| DataError::Engine("worker"))??,
            _ = second.tick(), if ticking => line.update(&format!("# scanning {}s", scan.elapsed().as_secs())),
            _ = tokio::time::sleep_until(deadline) => { cancel.cancel(); let _ = worker.await; return Err(DataError::Timeout(scanned.context(start)).into()); }
            _ = interrupt.recv() => { cancel.cancel(); let _ = worker.await; return Err(DataError::Interrupted(scanned.context(start)).into()); }
        }
    };
    drop(line);
    if let Some(clients) = &mut clients {
        tokio::select! {
            result = tokio::time::timeout_at(deadline, s3_data::verify_unchanged(clients, &files)) => result.map_err(|_| DataError::Timeout(scanned.context(start)))??,
            _ = interrupt.recv() => return Err(DataError::Interrupted(scanned.context(start)).into()),
        }
    }
    output.elapsed_ms = Some(start.elapsed().as_millis() as u64);
    // Either the run was measured or it was not: a request count without the
    // engine's own reads would be a number that looks complete and is not.
    if let (Some(clients), Some(transfer)) = (&clients, &completed.transfer) {
        output.meta.request_count = Some(clients.requests() + transfer.requests);
        output.meta.bytes_transferred = Some(transfer.bytes);
    }
    let incomplete = record_result(&mut output, completed.result);
    let projection = command
        .jq
        .as_deref()
        .map(|filter| project_output(&output, filter))
        .transpose()?;
    if let Some((file, path)) = completed.export.take() {
        file.as_file().sync_all().map_err(DataError::Io)?;
        file.persist_noclobber(path)
            .map_err(|e| DataError::Io(e.error))?;
    }
    match projection {
        Some(value) => println!("{value}"),
        None => print_result(&output, command.json),
    }
    if incomplete {
        return Err(DataError::Incomplete.into());
    }
    Ok(())
}

/// Put the engine's result into the envelope with the advice it calls for,
/// and say whether it is partial: truncated, unless a preview stopped at the
/// rows it was asked for.
fn record_result(output: &mut DataOutput, payload: DataPayload) -> bool {
    let incomplete = payload.result.as_ref().is_some_and(|result| {
        result.truncated
            && !(output.operation == DataOperation::Preview
                && result.stop_reason.as_deref() == Some("max_rows"))
    });
    if let Some(result) = &payload.result {
        output.meta.returned_rows = Some(result.row_count);
        output.meta.returned_result_bytes = Some(result.result_bytes);
        if result.truncated {
            output.meta.next_actions.push(NextAction {
                kind: DataKind::Data, operation: DataOperation::Query,
                message: "Use --query with a narrower SELECT, raise the display limits, or use --query with --export for all rows; there is no continuation cursor.".into(),
            });
        }
    }
    if let Some(parquet) = &payload.parquet {
        output
            .meta
            .next_actions
            .extend(parquet_advice::projection_advice(parquet));
    }
    output.payload = Some(payload);
    incomplete
}

/// The region every bucket was read in, or each distinct one when the buckets
/// live in more than one region.
fn read_regions(regions: &BTreeMap<String, String>) -> Option<String> {
    let distinct: BTreeSet<&str> = regions.values().map(String::as_str).collect();
    (!distinct.is_empty()).then(|| distinct.into_iter().collect::<Vec<_>>().join(", "))
}

fn project_output(output: &DataOutput, filter: &str) -> Result<Value, DataError> {
    crate::shell::cli::client::project_document(output, filter)
        .map_err(|_| DataError::Invalid(InvalidInput::InvalidProjection))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_reports_the_region_of_every_bucket_read() {
        let regions = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(bucket, region)| ((*bucket).to_owned(), (*region).to_owned()))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(read_regions(&regions(&[])), None);
        assert_eq!(
            read_regions(&regions(&[("west", "us-west-2"), ("other", "us-west-2")])).as_deref(),
            Some("us-west-2")
        );
        assert_eq!(
            read_regions(&regions(&[
                ("west", "us-west-2"),
                ("tokyo", "ap-northeast-1")
            ]))
            .as_deref(),
            Some("ap-northeast-1, us-west-2")
        );
    }
}
