//! `kurama s3`: resolve the connection and start, assume the role once, list, search or read to a bound, print one result.
use super::{
    s3_command::S3Command,
    s3_read::{self, PreviewRequest},
};
use crate::{
    adapters::{
        aws::{s3_browse, s3_data::BucketClients},
        config::{Config, data::S3Connection},
    },
    domain::{
        functions::{
            s3_handoff::{Observed, data_handoff},
            s3_scan::{Scan, Scanned, key_contains},
        },
        types::{
            limits::S3_READ,
            s3_browse::{
                DEFAULT_PAGE_SIZE, DEFAULT_REQUEST_TIMEOUT_SECS, DEFAULT_SEARCH_OBJECTS, S3Cursor,
                S3Entry, S3Error, S3Invalid, S3Location, S3Operation, S3Output, S3Position,
                S3Result, S3StopReason,
            },
        },
    },
    ports::AwsProfileCredentials,
    shell::{
        aws_profile_credentials::{AssumeRoleCredentials, AssumedRoles},
        cli::client::{json_line, tab_separated},
    },
};
use serde_json::Value;
use std::sync::Arc;

/// What one run reads: where, how, and from which position.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    operation: S3Operation,
    location: Option<S3Location>,
    search: Option<String>,
    recursive: bool,
    page_size: u32,
    max_objects: Option<u64>,
    start: S3Position,
    /// `--preview`: the bytes shown and where they start.
    bytes: Option<u64>,
    offset: Option<u64>,
}

pub async fn run(command: S3Command, config: Config) -> anyhow::Result<()> {
    let connection = config
        .s3
        .get(&command.s3)
        .cloned()
        .ok_or_else(|| S3Invalid::UnknownConnection(command.s3.clone()))?;
    // An agent or a pipe gets the usage error instead of a blank screen.
    if command.operation.is_none()
        && !command.json
        && !command.dry_run
        && crate::shell::tui::terminal::supports_tui()
    {
        return explore(&command, &connection, config).await;
    }
    let plan = plan(&command, &connection)?;
    let (auth, profile, region) = signing(&command, &connection, config).await?;
    let mut output = S3Output {
        schema_version: 1,
        kind: "s3",
        operation: plan.operation,
        s3: command.s3.clone(),
        bucket: plan.location.as_ref().map(|l| l.bucket.clone()),
        prefix: plan.location.as_ref().map(|l| l.prefix.clone()),
        recursive: plan.recursive,
        search: plan.search.clone(),
        aws_profile: connection.aws_profile.clone(),
        region: region.clone(),
        page_size: plan.page_size,
        max_objects: plan.max_objects,
        bytes: plan.bytes,
        offset_bytes: plan.offset,
        if_match: command.if_match.clone(),
        dry_run: None,
        result: None,
    };
    if command.dry_run {
        output.dry_run = Some(true);
        print_output(&output, command.json);
        return Ok(());
    }
    let credentials = auth.assume_role(&profile).await?;
    let timeout = connection
        .request_timeout_secs
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
    // The profile's or the named region only says where the first request is
    // signed: a bucket elsewhere answers with its own, which is followed.
    let mut clients = BucketClients::new(&credentials, &region, true, timeout);
    let mut result = match (&plan.location, plan.operation) {
        (Some(location), S3Operation::Head) => {
            s3_read::head(&mut clients, &location.bucket, &location.prefix).await?
        }
        (Some(location), S3Operation::Preview) => {
            let request = PreviewRequest {
                bucket: &location.bucket,
                key: &location.prefix,
                bytes: plan.bytes.unwrap_or(S3_READ.preview_bytes),
                offset: plan.offset.unwrap_or(0),
                if_match: command.if_match.as_deref(),
            };
            s3_read::preview(&mut clients, request).await?
        }
        (Some(location), S3Operation::ContentSearch) => {
            let text = plan.search.as_deref().expect("a content search has a text");
            let scanned = scan(&mut clients, &plan, location, |entry| {
                matches!(entry, S3Entry::Object(_))
            })
            .await?;
            let run = s3_read::search_contents(
                &mut clients,
                &location.bucket,
                &scanned.objects,
                text,
                || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            )
            .await?;
            let complete = scanned.resume.is_none()
                && run.stop.is_none()
                && run.skipped.is_empty()
                && run.searched.iter().all(|s| s.full);
            let listed = listing_result(&plan, location, scanned);
            S3Result {
                objects: vec![],
                complete,
                stop_reason: run.stop.or(listed.stop_reason),
                matches: Some(run.matches),
                searched: Some(run.searched),
                skipped: Some(run.skipped),
                bytes_read: Some(run.bytes_read),
                bytes_decoded: Some(run.bytes_decoded),
                ..listed
            }
        }
        (Some(location), _) => {
            let scanned = match &plan.search {
                Some(text) => {
                    scan(&mut clients, &plan, location, |entry| {
                        key_contains(entry, text)
                    })
                    .await?
                }
                None => scan(&mut clients, &plan, location, |_| true).await?,
            };
            listing_result(&plan, location, scanned)
        }
        (None, _) => {
            let buckets = s3_browse::list_buckets(&mut clients, plan.page_size).await?;
            S3Result {
                scanned_objects: buckets.len() as u64,
                buckets: Some(buckets),
                complete: true,
                ..S3Result::default()
            }
        }
    };
    if let (Some(location), Some(object)) = (&plan.location, &result.object) {
        let observed = Observed {
            bucket: &location.bucket,
            key: &object.key,
            etag: object.etag.as_deref(),
            size: object.size,
            last_modified: object.last_modified.as_deref(),
        };
        result.next_actions = Some(data_handoff(&command.s3, &observed).into_iter().collect());
    }
    if let Some(location) = &plan.location
        && let Some(read) = clients.regions().get(&location.bucket)
    {
        output.region = read.clone();
    }
    output.result = Some(result);
    print_output(&output, command.json);
    Ok(())
}

/// The role's profile and the region the first request is signed for:
/// `--region`, the `[s3.*]`, then the AWS profile.
async fn signing(
    command: &S3Command,
    connection: &S3Connection,
    config: Config,
) -> anyhow::Result<(AssumeRoleCredentials, crate::domain::Profile, String)> {
    let auth = AssumeRoleCredentials::new(Arc::new(config));
    let profile = auth.load_profile(&connection.aws_profile).await?;
    let region = command
        .region
        .clone()
        .or_else(|| connection.region.clone())
        .or_else(|| profile.region_raw().map(str::to_owned))
        .ok_or(S3Invalid::RegionRequired)?;
    Ok((auth, profile, region))
}

/// `kurama s3 <S3>` on a terminal with no operation: the explorer, from
/// TARGET, the `[s3.*]` bucket and prefix, or the bucket list. The role is
/// assumed before the screen opens, so a prompt is never under it, and again
/// by the explorer's task only when those credentials are about to end.
async fn explore(
    command: &S3Command,
    connection: &S3Connection,
    config: Config,
) -> anyhow::Result<()> {
    let start = match &command.target {
        Some(target) => Some(S3Location::parse(target)?),
        None => connection.bucket.clone().map(|bucket| S3Location {
            bucket,
            prefix: connection.prefix.clone().unwrap_or_default(),
        }),
    };
    let (auth, profile, region) = signing(command, connection, config).await?;
    let roles: Arc<dyn AwsProfileCredentials> = Arc::new(AssumedRoles::new(Arc::new(auth)));
    let credentials = roles.assume_role(&profile).await?;
    let timeout = connection
        .request_timeout_secs
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
    let clients = BucketClients::new(&credentials, &region, true, timeout);
    let role = crate::shell::tui::s3::Role {
        credentials: roles,
        profile,
    };
    let summary = crate::shell::tui::s3::model::S3Summary {
        name: command.s3.clone(),
        aws_profile: connection.aws_profile.clone(),
        region,
        page_size: connection.page_size.unwrap_or(DEFAULT_PAGE_SIZE),
    };
    crate::shell::tui::s3::handle_s3_explorer(summary, start, clients, role).await
}

/// List from the plan's start to its bound, keeping what `keep` accepts.
async fn scan(
    clients: &mut BucketClients,
    plan: &Plan,
    location: &S3Location,
    keep: impl Fn(&S3Entry) -> bool,
) -> Result<Scanned, S3Error> {
    let max_objects = plan.max_objects.expect("a listing has a bound");
    let mut scan = Scan::new(plan.start.clone(), max_objects);
    let delimited = plan.operation == S3Operation::List && !plan.recursive;
    while let Some(token) = scan.next_page() {
        let page = s3_browse::list_page(
            clients,
            &location.bucket,
            &location.prefix,
            delimited,
            plan.page_size,
            token,
        )
        .await?;
        scan.accept(page, &keep);
    }
    Ok(scan.finish())
}

/// What a listing found, whether it is complete, and the cursor that goes on.
fn listing_result(plan: &Plan, location: &S3Location, scanned: Scanned) -> S3Result {
    S3Result {
        prefixes: scanned.prefixes,
        objects: scanned.objects,
        scanned_objects: scanned.scanned,
        complete: scanned.resume.is_none(),
        stop_reason: scanned.resume.as_ref().map(|_| S3StopReason::MaxObjects),
        cursor: scanned.resume.map(|position| {
            S3Cursor {
                bucket: location.bucket.clone(),
                prefix: location.prefix.clone(),
                operation: plan.operation,
                recursive: plan.recursive,
                search: plan.search.clone(),
                page_size: plan.page_size,
                position,
            }
            .encode()
        }),
        ..S3Result::default()
    }
}

/// The operation, the start and the bounds, from the arguments, the
/// `[s3.*]` and a cursor, before anything is read.
fn plan(command: &S3Command, connection: &S3Connection) -> Result<Plan, S3Invalid> {
    let operation = command.operation.ok_or(S3Invalid::OperationRequired)?;
    if command.search.as_deref() == Some("") {
        return Err(S3Invalid::EmptySearch);
    }
    let mut page_size = connection.page_size.unwrap_or(DEFAULT_PAGE_SIZE);
    if operation == S3Operation::Buckets {
        return Ok(Plan {
            operation,
            location: None,
            search: None,
            recursive: false,
            page_size,
            max_objects: None,
            start: S3Position::default(),
            bytes: None,
            offset: None,
        });
    }
    let location = match &command.target {
        Some(target) => S3Location::parse(target)?,
        None => S3Location {
            bucket: connection.bucket.clone().ok_or(S3Invalid::BucketRequired)?,
            prefix: connection.prefix.clone().unwrap_or_default(),
        },
    };
    if matches!(operation, S3Operation::Head | S3Operation::Preview) {
        if location.prefix.is_empty() || location.prefix.ends_with('/') {
            return Err(S3Invalid::KeyRequired);
        }
        let preview = operation == S3Operation::Preview;
        return Ok(Plan {
            operation,
            location: Some(location),
            search: None,
            recursive: false,
            page_size,
            max_objects: None,
            start: S3Position::default(),
            bytes: preview.then(|| command.bytes.unwrap_or(S3_READ.preview_bytes)),
            offset: preview.then(|| command.offset_bytes.unwrap_or(0)),
        });
    }
    let mut start = S3Position::default();
    if let Some(text) = &command.cursor {
        let cursor = S3Cursor::decode(text)?;
        if !cursor.continues(
            &location,
            operation,
            command.recursive,
            command.search.as_deref(),
        ) {
            return Err(S3Invalid::CursorMismatch);
        }
        // The token names a page of this size; another size skips into
        // another page.
        page_size = cursor.page_size;
        start = cursor.position;
    }
    let max_objects = command.max_objects.unwrap_or(match operation {
        S3Operation::Search => DEFAULT_SEARCH_OBJECTS,
        S3Operation::ContentSearch => S3_READ.search_objects,
        _ => u64::from(page_size),
    });
    Ok(Plan {
        operation,
        location: Some(location),
        search: command.search.clone(),
        recursive: command.recursive,
        page_size,
        max_objects: Some(max_objects),
        start,
        bytes: None,
        offset: None,
    })
}

fn print_output(output: &S3Output, json: bool) {
    if json {
        println!("{}", json_line(output));
        return;
    }
    let Some(result) = &output.result else {
        println!(
            "{}",
            serde_json::to_string_pretty(output).expect("the plan serializes")
        );
        return;
    };
    if matches!(
        output.operation,
        S3Operation::Head | S3Operation::Preview | S3Operation::ContentSearch
    ) {
        s3_read::print_text(result);
        return;
    }
    if let Some(buckets) = &result.buckets {
        let rows: Vec<Vec<Value>> = buckets
            .iter()
            .map(|b| {
                vec![
                    b.name.clone().into(),
                    b.region.clone().unwrap_or_default().into(),
                    b.created.clone().unwrap_or_default().into(),
                ]
            })
            .collect();
        print!(
            "{}",
            tab_separated(["name", "region", "created"].into_iter(), &rows)
        );
        return;
    }
    let rows: Vec<Vec<Value>> = result
        .prefixes
        .iter()
        .map(|p| vec!["prefix".into(), p.clone().into(), "".into(), "".into()])
        .chain(result.objects.iter().map(|o| {
            vec![
                "object".into(),
                o.key.clone().into(),
                o.size.into(),
                o.last_modified.clone().unwrap_or_default().into(),
            ]
        }))
        .collect();
    print!(
        "{}",
        tab_separated(["type", "key", "size", "last_modified"].into_iter(), &rows)
    );
    if let Some(cursor) = &result.cursor {
        crate::console::progress!(
            "# stopped after {} entries (max_objects); go on with --cursor {cursor}",
            result.scanned_objects
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> S3Connection {
        S3Connection {
            aws_profile: "dev".into(),
            region: None,
            bucket: Some("configured".into()),
            prefix: Some("reports/".into()),
            page_size: Some(3),
            request_timeout_secs: None,
        }
    }

    fn command(operation: Option<S3Operation>) -> S3Command {
        S3Command {
            s3: "assets".into(),
            target: None,
            operation,
            search: None,
            recursive: false,
            cursor: None,
            max_objects: None,
            region: None,
            dry_run: false,
            json: true,
            bytes: None,
            offset_bytes: None,
            if_match: None,
        }
    }

    #[test]
    fn s3_plan_starts_from_the_connection_and_bounds_each_operation() {
        let list = plan(&command(Some(S3Operation::List)), &connection()).unwrap();
        assert_eq!(
            list.location,
            Some(S3Location {
                bucket: "configured".into(),
                prefix: "reports/".into()
            })
        );
        assert_eq!((list.page_size, list.max_objects), (3, Some(3)));
        let search = plan(
            &S3Command {
                search: Some("inv".into()),
                target: Some("s3://other/a//b/".into()),
                ..command(Some(S3Operation::Search))
            },
            &connection(),
        )
        .unwrap();
        assert_eq!(search.location.unwrap().prefix, "a//b/");
        assert_eq!(search.max_objects, Some(DEFAULT_SEARCH_OBJECTS));
        let buckets = plan(&command(Some(S3Operation::Buckets)), &connection()).unwrap();
        assert_eq!((buckets.location, buckets.max_objects), (None, None));
    }

    #[test]
    fn s3_plan_refuses_what_it_cannot_read() {
        assert!(matches!(
            plan(&command(None), &connection()),
            Err(S3Invalid::OperationRequired)
        ));
        let bare = S3Connection {
            bucket: None,
            prefix: None,
            ..connection()
        };
        assert!(matches!(
            plan(&command(Some(S3Operation::List)), &bare),
            Err(S3Invalid::BucketRequired)
        ));
        let empty = S3Command {
            search: Some(String::new()),
            ..command(Some(S3Operation::Search))
        };
        assert!(matches!(
            plan(&empty, &connection()),
            Err(S3Invalid::EmptySearch)
        ));
    }

    #[test]
    fn s3_plan_reads_one_object_and_bounds_a_content_search() {
        for target in ["s3://b/", "s3://b", "s3://b/logs/"] {
            for operation in [S3Operation::Head, S3Operation::Preview] {
                let command = S3Command {
                    target: Some(target.into()),
                    ..command(Some(operation))
                };
                assert!(
                    matches!(plan(&command, &connection()), Err(S3Invalid::KeyRequired)),
                    "{target}"
                );
            }
        }
        let preview = plan(
            &S3Command {
                target: Some("s3://b/r.json".into()),
                offset_bytes: Some(9),
                ..command(Some(S3Operation::Preview))
            },
            &connection(),
        )
        .unwrap();
        assert_eq!(
            (preview.bytes, preview.offset, preview.max_objects),
            (Some(S3_READ.preview_bytes), Some(9), None)
        );
        let content = plan(
            &S3Command {
                search: Some("id".into()),
                ..command(Some(S3Operation::ContentSearch))
            },
            &connection(),
        )
        .unwrap();
        assert_eq!(content.max_objects, Some(S3_READ.search_objects));
    }

    #[test]
    fn s3_plan_resumes_from_a_cursor_with_its_page_size() {
        let cursor = S3Cursor {
            bucket: "configured".into(),
            prefix: "reports/".into(),
            operation: S3Operation::List,
            recursive: true,
            search: None,
            page_size: 7,
            position: S3Position {
                token: Some("t".into()),
                skip: 2,
            },
        };
        let resumed = S3Command {
            recursive: true,
            cursor: Some(cursor.encode()),
            ..command(Some(S3Operation::List))
        };
        let planned = plan(&resumed, &connection()).unwrap();
        assert_eq!(
            (planned.page_size, planned.start),
            (7, cursor.position.clone())
        );
        let other = S3Command {
            recursive: false,
            ..resumed
        };
        assert!(matches!(
            plan(&other, &connection()),
            Err(S3Invalid::CursorMismatch)
        ));
    }
}
