//! The disposable AWS stacks a `[throwaway]` case can name: which script creates and removes each, what it costs, how long it takes, and which of its outputs the case reads through a placeholder.

use std::collections::BTreeMap;

/// One stack: the scripts under `tests/` own the template and the waiting;
/// this only says what they are and what they leave in the stack's outputs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Stack {
    /// The name a case writes in `stacks = [..]`.
    pub(crate) name: &'static str,
    /// The environment variable the script reads the stack name from, and
    /// the name it uses when that is unset (a person's own run); a run of
    /// xtask names the stack after this default and its own id.
    pub(crate) stack_variable: &'static str,
    pub(crate) default_stack_name: &'static str,
    pub(crate) up: &'static str,
    pub(crate) down: &'static str,
    /// From the script headers: what it bills while it exists, and how long
    /// creating and deleting it take. A person reads these before `--yes`.
    pub(crate) cost_per_hour_usd: f64,
    pub(crate) creation: &'static str,
    pub(crate) deletion: &'static str,
    /// `(OutputKey, environment variable)`: the outputs the case's `[throwaway]`
    /// config reads, each through the placeholder `tests/support/cases.rs`
    /// maps to that variable.
    pub(crate) outputs: &'static [(&'static str, &'static str)],
    /// A second stack `up` makes in another Region, which `down` removes
    /// and a run checks is gone like the first.
    pub(crate) other_region: Option<OtherRegion>,
}

/// The stack `<name><suffix>` a script makes in the Region `variable` names,
/// `default` when it is unset (`s3-data`: `-other` in `KURAMA_S3_OTHER_REGION`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct OtherRegion {
    pub(crate) suffix: &'static str,
    pub(crate) variable: &'static str,
    pub(crate) default: &'static str,
}

/// One CloudFormation stack of a run: its name, and its Region when that is
/// not the profile's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloudFormationStack {
    pub(crate) name: String,
    pub(crate) region: Option<String>,
}

impl std::fmt::Display for CloudFormationStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.region {
            Some(region) => write!(f, "{} in {region}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

/// The variable that carries the run's id to every script: `s3-up.sh` puts
/// it in the bucket names, which are global to the account.
pub(crate) const RUN_VARIABLE: &str = "KURAMA_THROWAWAY_RUN";

/// The variable that carries the stack's own name to the case (its
/// `instance_name` for a tunnel is the stack name, by the templates' tag).
pub(crate) fn stack_name_variable(stack: &Stack) -> Option<&'static str> {
    match stack.name {
        "bastion" => Some("KURAMA_STACK_BASTION_STACK"),
        "rds-iam" => Some("KURAMA_STACK_RDS_IAM_STACK"),
        _ => None,
    }
}

pub(crate) const STACKS: [Stack; 5] = [
    Stack {
        name: "iam-api",
        stack_variable: "KURAMA_IAM_API_STACK",
        default_stack_name: "kurama-iam-api-test",
        up: "tests/api/iam-api-up.sh",
        down: "tests/api/iam-api-down.sh",
        cost_per_hour_usd: 0.0,
        creation: "about a minute",
        deletion: "under a minute",
        outputs: &[
            ("RestApiUrl", "KURAMA_STACK_IAM_API_URL"),
            ("FunctionUrl", "KURAMA_STACK_LAMBDA_URL"),
        ],
        other_region: None,
    },
    Stack {
        name: "bastion",
        stack_variable: "KURAMA_BASTION_STACK",
        default_stack_name: "kurama-tunnel-test",
        up: "tests/db/bastion-up.sh",
        down: "tests/db/bastion-down.sh",
        cost_per_hour_usd: 0.012,
        creation: "five to ten minutes (the database is installed at first boot)",
        deletion: "a minute or two",
        outputs: &[("InstanceId", "KURAMA_STACK_BASTION_INSTANCE_ID")],
        other_region: None,
    },
    Stack {
        name: "rds-iam",
        stack_variable: "KURAMA_RDS_IAM_STACK",
        default_stack_name: "kurama-rds-iam-test",
        up: "tests/db/rds-iam-up.sh",
        down: "tests/db/rds-iam-down.sh",
        cost_per_hour_usd: 0.06,
        creation: "about ten minutes",
        deletion: "about ten minutes",
        outputs: &[
            ("PostgresHost", "KURAMA_STACK_RDS_PG_HOST"),
            ("MysqlHost", "KURAMA_STACK_RDS_MYSQL_HOST"),
        ],
        other_region: None,
    },
    // DSQL bills DPUs per request and storage, inside a free tier of
    // 100,000 DPU-hours a month; a cluster a check leaves idle is about $0.
    Stack {
        name: "dsql",
        stack_variable: "KURAMA_DSQL_STACK",
        default_stack_name: "kurama-dsql-test",
        up: "tests/db/dsql-up.sh",
        down: "tests/db/dsql-down.sh",
        cost_per_hour_usd: 0.0,
        creation: "two to five minutes (the cluster is polled until ACTIVE)",
        deletion: "a minute or two",
        outputs: &[
            ("Endpoint", "KURAMA_STACK_DSQL_HOST"),
            ("Identifier", "KURAMA_STACK_DSQL_IDENTIFIER"),
        ],
        other_region: None,
    },
    // Two stacks of one template, one per Region, so the second bucket lives
    // outside the profile's Region; the first stack's outputs name both.
    Stack {
        name: "s3-data",
        stack_variable: "KURAMA_S3_DATA_STACK",
        default_stack_name: "kurama-s3-data-test",
        up: "tests/s3/s3-up.sh",
        down: "tests/s3/s3-down.sh",
        cost_per_hour_usd: 0.0,
        creation: "about a minute (two stacks, one per Region, and the fixtures uploaded to both)",
        deletion: "under a minute (both buckets emptied first)",
        outputs: &[
            ("BucketName", "KURAMA_STACK_S3_BUCKET_SAME_REGION"),
            ("BucketRegion", "KURAMA_STACK_S3_SAME_REGION"),
            ("OtherBucket", "KURAMA_STACK_S3_BUCKET_OTHER_REGION"),
            ("OtherRegion", "KURAMA_STACK_S3_OTHER_REGION"),
        ],
        other_region: Some(OtherRegion {
            suffix: "-other",
            variable: "KURAMA_S3_OTHER_REGION",
            default: "us-west-2",
        }),
    },
];

pub(crate) fn find(name: &str) -> Option<&'static Stack> {
    STACKS.iter().find(|stack| stack.name == name)
}

/// The stack's name for this run: the scripts' default with the run's id, so
/// neither a person's own stack of the default name nor another run's is
/// the one this run uses and deletes.
pub(crate) fn stack_name(stack: &Stack, run: &str) -> String {
    format!("{}-{run}", stack.default_stack_name)
}

/// Every CloudFormation stack `up` makes for `stack` in this run, the one of
/// its name first.
pub(crate) fn cloudformation_stacks(stack: &Stack, run: &str) -> Vec<CloudFormationStack> {
    let name = stack_name(stack, run);
    let mut stacks = vec![CloudFormationStack {
        name: name.clone(),
        region: None,
    }];
    if let Some(other) = &stack.other_region {
        stacks.push(CloudFormationStack {
            name: format!("{name}{}", other.suffix),
            region: Some(other_region(other)),
        });
    }
    stacks
}

fn other_region(other: &OtherRegion) -> String {
    std::env::var(other.variable).unwrap_or_else(|_| other.default.to_string())
}

/// What every script of the run is handed: each stack's name, the run's id,
/// and the other Region -- the same values `cleanup.sh` writes down, so a
/// person running it later removes exactly these stacks.
pub(crate) fn script_variables(stacks: &[&Stack], run: &str) -> Vec<(String, String)> {
    let mut variables = vec![(RUN_VARIABLE.to_string(), run.to_string())];
    for stack in stacks {
        variables.push((stack.stack_variable.to_string(), stack_name(stack, run)));
        if let Some(other) = &stack.other_region {
            variables.push((other.variable.to_string(), other_region(other)));
        }
    }
    variables
}

/// The stacks the cases need, in catalogue order, each once; a name no
/// stack has is an error naming the case that wrote it.
pub(crate) fn needed<'a>(
    cases: impl IntoIterator<Item = (&'a str, &'a [String])>,
) -> Result<Vec<&'static Stack>, String> {
    let mut names: Vec<&str> = Vec::new();
    for (case, stacks) in cases {
        for name in stacks {
            if find(name).is_none() {
                return Err(format!(
                    "{case}: `stacks` names `{name}`, which is none of {}",
                    STACKS
                        .iter()
                        .map(|stack| stack.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !names.contains(&name.as_str()) {
                names.push(name);
            }
        }
    }
    Ok(STACKS
        .iter()
        .filter(|stack| names.contains(&stack.name))
        .collect())
}

/// What `--yes` would create, for a person to read first: each stack with
/// what it bills and how long it takes, and the total per hour.
pub(crate) fn estimate(stacks: &[&Stack]) -> String {
    let mut out = String::new();
    for stack in stacks {
        out.push_str(&format!(
            "  {:<8} ${:.3}/hour  create {}, delete {}  ({} / {})\n",
            stack.name,
            stack.cost_per_hour_usd,
            stack.creation,
            stack.deletion,
            stack.up,
            stack.down
        ));
    }
    let total: f64 = stacks.iter().map(|stack| stack.cost_per_hour_usd).sum();
    out.push_str(&format!(
        "  total    ${total:.3}/hour while the run lasts; every stack is deleted when it ends\n"
    ));
    out
}

/// The variables a case's `[throwaway]` reads, from the outputs
/// `describe-stacks --query Stacks[0].Outputs --output json` printed for
/// the stack: an output the stack promises but AWS did not print is an
/// error, so a case never reads a placeholder as its own text.
pub(crate) fn output_variables(
    stack: &Stack,
    name: &str,
    describe_json: &str,
) -> Result<BTreeMap<String, String>, String> {
    let outputs: Vec<serde_json::Value> = serde_json::from_str(describe_json)
        .map_err(|e| format!("{}: the stack outputs are not JSON: {e}", stack.name))?;
    let value_of = |key: &str| {
        outputs
            .iter()
            .find(|output| output["OutputKey"] == key)
            .and_then(|output| output["OutputValue"].as_str())
            .map(str::to_string)
    };
    let mut variables = BTreeMap::new();
    for (key, variable) in stack.outputs {
        let value = value_of(key).ok_or_else(|| {
            format!(
                "{}: the stack printed no `{key}` output; the template and this catalogue disagree",
                stack.name
            )
        })?;
        variables.insert((*variable).to_string(), value);
    }
    if let Some(variable) = stack_name_variable(stack) {
        variables.insert(variable.to_string(), name.to_string());
    }
    Ok(variables)
}

/// What `describe-stacks` said, before `up` and after `down`: absent when the
/// API answers that the stack does not exist, otherwise the status it reports.
pub(crate) fn absence_verdict(exit_ok: bool, stdout: &str, stderr: &str) -> Result<(), String> {
    if !exit_ok && stderr.contains("does not exist") {
        return Ok(());
    }
    let status = serde_json::from_str::<serde_json::Value>(stdout)
        .ok()
        .and_then(|value| {
            value["Stacks"][0]["StackStatus"]
                .as_str()
                .map(str::to_string)
        });
    Err(match status {
        Some(status) if status == "DELETE_COMPLETE" => return Ok(()),
        Some(status) => format!("reported as {status}"),
        None if exit_ok => "reported".to_string(),
        None => format!(
            "describe-stacks failed: {}",
            stderr.lines().last().unwrap_or("").trim()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needed_stacks_come_once_in_catalogue_order_and_an_unknown_one_is_named() {
        let a = vec!["rds-iam".to_string(), "iam-api".to_string()];
        let b = vec!["iam-api".to_string()];
        let stacks = needed([("x", a.as_slice()), ("y", b.as_slice())]).unwrap();
        assert_eq!(
            stacks.iter().map(|stack| stack.name).collect::<Vec<_>>(),
            ["iam-api", "rds-iam"]
        );
        let bogus = vec!["moon".to_string()];
        let error = needed([("z", bogus.as_slice())]).unwrap_err();
        assert!(error.starts_with("z: `stacks` names `moon`"), "{error}");
        assert!(
            error.ends_with("iam-api, bastion, rds-iam, dsql, s3-data"),
            "{error}"
        );
    }

    /// The two stacks of the S3 and DSQL cases: the cluster's endpoint and
    /// identifier, and the bucket in each Region with its Region, each an
    /// output the case reads through a placeholder; a stack of the wrong
    /// template is caught
    /// by the output it lacks.
    #[test]
    fn the_dsql_and_s3_stacks_name_their_outputs() {
        let dsql = find("dsql").unwrap();
        let variables = output_variables(
            dsql,
            "kurama-dsql-test-1",
            r#"[{"OutputKey":"Endpoint","OutputValue":"abcdefghijklmnopqrst.dsql.ap-northeast-1.on.aws"},
               {"OutputKey":"Identifier","OutputValue":"abcdefghijklmnopqrst"}]"#,
        )
        .unwrap();
        assert_eq!(
            variables["KURAMA_STACK_DSQL_HOST"],
            "abcdefghijklmnopqrst.dsql.ap-northeast-1.on.aws"
        );
        assert_eq!(
            variables["KURAMA_STACK_DSQL_IDENTIFIER"],
            "abcdefghijklmnopqrst"
        );
        assert_eq!(
            dsql.cost_per_hour_usd, 0.0,
            "an idle cluster inside the free tier"
        );

        let s3 = find("s3-data").unwrap();
        let variables = output_variables(
            s3,
            "kurama-s3-data-test-1",
            r#"[{"OutputKey":"BucketName","OutputValue":"kurama-data-test-123456789012-same"},
               {"OutputKey":"BucketRegion","OutputValue":"ap-northeast-1"},
               {"OutputKey":"OtherBucket","OutputValue":"kurama-data-test-123456789012-other"},
               {"OutputKey":"OtherRegion","OutputValue":"us-west-2"}]"#,
        )
        .unwrap();
        assert_eq!(
            variables["KURAMA_STACK_S3_BUCKET_SAME_REGION"],
            "kurama-data-test-123456789012-same"
        );
        assert_eq!(variables["KURAMA_STACK_S3_SAME_REGION"], "ap-northeast-1");
        assert_eq!(
            variables["KURAMA_STACK_S3_BUCKET_OTHER_REGION"],
            "kurama-data-test-123456789012-other"
        );
        assert_eq!(variables["KURAMA_STACK_S3_OTHER_REGION"], "us-west-2");
        let error = output_variables(
            s3,
            "kurama-s3-data-test-1",
            r#"[{"OutputKey":"BucketName","OutputValue":"b"},{"OutputKey":"BucketRegion","OutputValue":"r"}]"#,
        )
        .unwrap_err();
        assert!(error.contains("no `OtherBucket` output"), "{error}");
        for stack in [dsql, s3] {
            for script in [stack.up, stack.down] {
                assert!(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("..")
                        .join(script)
                        .is_file(),
                    "{script} exists"
                );
            }
        }
    }

    /// A run names every stack after its own id and hands the scripts those
    /// names, the id and the other Region; `s3-data` is two stacks, the
    /// second in that Region.
    #[test]
    fn a_run_names_its_stacks_and_hands_the_names_to_the_scripts() {
        let s3 = find("s3-data").unwrap();
        let iam = find("iam-api").unwrap();
        assert_eq!(
            cloudformation_stacks(iam, "9-1")
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["kurama-iam-api-test-9-1"]
        );
        assert_eq!(
            cloudformation_stacks(s3, "9-1")
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            [
                "kurama-s3-data-test-9-1",
                "kurama-s3-data-test-9-1-other in us-west-2"
            ]
        );
        assert_eq!(
            script_variables(&[iam, s3], "9-1"),
            [
                ("KURAMA_THROWAWAY_RUN", "9-1"),
                ("KURAMA_IAM_API_STACK", "kurama-iam-api-test-9-1"),
                ("KURAMA_S3_DATA_STACK", "kurama-s3-data-test-9-1"),
                ("KURAMA_S3_OTHER_REGION", "us-west-2"),
            ]
            .map(|(name, value)| (name.to_string(), value.to_string()))
        );
    }

    #[test]
    fn the_estimate_sums_the_cost_and_names_every_script() {
        let text = estimate(&[&STACKS[0], &STACKS[2]]);
        assert!(text.contains("iam-api  $0.000/hour"), "{text}");
        assert!(text.contains("rds-iam  $0.060/hour"), "{text}");
        assert!(text.contains("total    $0.060/hour"), "{text}");
        assert!(text.contains("tests/db/rds-iam-up.sh / tests/db/rds-iam-down.sh"));
    }

    #[test]
    fn outputs_become_the_variables_the_placeholders_read() {
        let json = r#"[{"OutputKey":"FunctionUrl","OutputValue":"https://f.lambda-url.ap-northeast-1.on.aws/"},
                       {"OutputKey":"RestApiUrl","OutputValue":"https://r.execute-api.ap-northeast-1.amazonaws.com/v1"}]"#;
        let variables = output_variables(&STACKS[0], "kurama-iam-api-test-1", json).unwrap();
        assert_eq!(
            variables["KURAMA_STACK_IAM_API_URL"],
            "https://r.execute-api.ap-northeast-1.amazonaws.com/v1"
        );
        assert_eq!(
            variables["KURAMA_STACK_LAMBDA_URL"],
            "https://f.lambda-url.ap-northeast-1.on.aws/"
        );
        let error = output_variables(
            &STACKS[0],
            "kurama-iam-api-test-1",
            r#"[{"OutputKey":"RestApiUrl","OutputValue":"x"}]"#,
        )
        .unwrap_err();
        assert!(error.contains("no `FunctionUrl` output"), "{error}");
        let bastion = output_variables(
            &STACKS[1],
            "kurama-tunnel-test-7",
            r#"[{"OutputKey":"InstanceId","OutputValue":"i-1"}]"#,
        )
        .unwrap();
        assert_eq!(bastion["KURAMA_STACK_BASTION_INSTANCE_ID"], "i-1");
        assert_eq!(
            bastion["KURAMA_STACK_BASTION_STACK"],
            "kurama-tunnel-test-7"
        );
    }

    #[test]
    fn an_absent_stack_is_the_one_aws_does_not_know() {
        assert!(absence_verdict(false, "", "An error occurred (ValidationError): Stack with id kurama-iam-api-test does not exist").is_ok());
        assert!(
            absence_verdict(
                true,
                r#"{"Stacks":[{"StackStatus":"DELETE_COMPLETE"}]}"#,
                ""
            )
            .is_ok()
        );
        assert_eq!(
            absence_verdict(true, r#"{"Stacks":[{"StackStatus":"DELETE_FAILED"}]}"#, "")
                .unwrap_err(),
            "reported as DELETE_FAILED"
        );
        assert_eq!(
            absence_verdict(false, "", "Unable to locate credentials").unwrap_err(),
            "describe-stacks failed: Unable to locate credentials"
        );
    }
}
