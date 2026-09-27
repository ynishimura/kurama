//! CLI arguments of `kurama s3`: the connection, the starting point, one operation and its bounds.
use crate::domain::types::{
    limits::S3_READ,
    s3_browse::{MAX_OBJECTS, S3Operation},
};
use crate::shell::cli::completion;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command, ValueHint};
use clap_complete::engine::ArgValueCandidates;

pub fn command() -> Command {
    Command::new("s3")
        .about("Walk a bucket and prefix, search keys or contents, or preview an object, with an [s3.*] connection's role")
        .long_about(
            "Walk a bucket and prefix, search keys or contents, or preview an object, with an\n\
             [s3.*] connection's role.\n\n\
             TARGET (s3://bucket/prefix) is read as written: %, +, spaces, .. and repeated / are\n\
             part of the key. Without it, the [s3.*] bucket and prefix are the start. --list\n\
             shows one level, or every key with --recursive; --search keeps the keys that contain\n\
             TEXT (case-sensitive, not a pattern). Each run examines at most --max-objects entries\n\
             and says whether it is complete; --cursor goes on where it stopped.\n\n\
             --head shows one object's metadata; --preview shows at most --bytes of it from\n\
             --offset-bytes (gzip decoded from the start, binary as a hex head); --search-content\n\
             reads the objects under the prefix to fixed bounds and reports each line that\n\
             contains TEXT. Contents are never written to disk.",
        )
        .arg(
            Arg::new("s3")
                .value_name("S3")
                .required(true)
                .help("An [s3.*] connection in config.toml")
                .add(ArgValueCandidates::new(completion::s3_connections)),
        )
        .arg(
            Arg::new("target")
                .value_name("TARGET")
                .value_hint(ValueHint::Other)
                .conflicts_with("buckets")
                .help("s3://bucket/prefix to start from; default: the [s3.*] bucket and prefix"),
        )
        .arg(
            Arg::new("buckets")
                .long("buckets")
                .action(ArgAction::SetTrue)
                .help("List every bucket the role may list (ListBuckets)"),
        )
        .arg(
            Arg::new("list")
                .long("list")
                .action(ArgAction::SetTrue)
                .help("List the common prefixes and objects under the prefix"),
        )
        .arg(
            Arg::new("search")
                .long("search")
                .value_name("TEXT")
                .value_hint(ValueHint::Other)
                .help("Keep the keys under the prefix that contain TEXT (case-sensitive literal)"),
        )
        .group(
            ArgGroup::new("operation")
                .args([
                    "buckets",
                    "list",
                    "search",
                    "head",
                    "preview",
                    "search-content",
                ])
                .multiple(false),
        )
        .arg(
            Arg::new("recursive")
                .long("recursive")
                .action(ArgAction::SetTrue)
                .requires("list")
                .conflicts_with_all(["search", "buckets", "head", "preview", "search-content"])
                .help("With --list: every key under the prefix, not one level"),
        )
        .arg(
            Arg::new("cursor")
                .long("cursor")
                .value_name("CURSOR")
                .value_hint(ValueHint::Other)
                .conflicts_with_all(["buckets", "head", "preview"])
                .help("Go on where the run that printed CURSOR stopped"),
        )
        .arg(
            Arg::new("max-objects")
                .long("max-objects")
                .value_name("N")
                .value_hint(ValueHint::Other)
                .value_parser(clap::value_parser!(u64).range(1..=MAX_OBJECTS))
                .conflicts_with_all(["buckets", "head", "preview"])
                .help("Keys and prefixes one run examines (default: the page size for --list, 1000 for --search, 100 for --search-content)"),
        )
        .arg(
            Arg::new("region")
                .long("region")
                .value_name("REGION")
                .value_hint(ValueHint::Other)
                .help("Region the first request is signed for (default: the [s3.*], then the AWS profile); the bucket's own region is followed"),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Show the target, operation and bounds without calling STS, S3 or 1Password"),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Print one JSON result and structured errors"),
        )
        .arg(
            Arg::new("head")
                .long("head")
                .action(ArgAction::SetTrue)
                .help("Show one object's size, type, ETag and storage class (HeadObject)"),
        )
        .arg(
            Arg::new("preview")
                .long("preview")
                .action(ArgAction::SetTrue)
                .help("Show at most --bytes of one object as text, or a hex head of a binary one"),
        )
        .arg(
            Arg::new("search-content")
                .long("search-content")
                .value_name("TEXT")
                .value_hint(ValueHint::Other)
                .help("Report the lines of the objects under the prefix that contain TEXT (case-sensitive literal)"),
        )
        .arg(
            Arg::new("bytes")
                .long("bytes")
                .value_name("N")
                .value_hint(ValueHint::Other)
                .value_parser(clap::value_parser!(u64).range(1..=S3_READ.max_preview_bytes))
                .requires("preview")
                // A requirement on one member of a group is met by any other.
                .conflicts_with_all(["buckets", "list", "search", "head", "search-content"])
                .help("With --preview: bytes shown (default 65536, at most 1048576)"),
        )
        .arg(
            Arg::new("offset-bytes")
                .long("offset-bytes")
                .value_name("N")
                .value_hint(ValueHint::Other)
                .value_parser(clap::value_parser!(u64))
                .requires("preview")
                // A requirement on one member of a group is met by any other.
                .conflicts_with_all(["buckets", "list", "search", "head", "search-content"])
                .help("With --preview: where the range starts (default 0; the previous run's next_offset goes on)"),
        )
        .arg(
            Arg::new("if-match")
                .long("if-match")
                .value_name("ETAG")
                .value_hint(ValueHint::Other)
                .requires("preview")
                // A requirement on one member of a group is met by any other.
                .conflicts_with_all(["buckets", "list", "search", "head", "search-content"])
                .help("With --preview: read only while the object's ETag is still ETAG"),
        )
}

#[derive(Debug, Clone)]
pub struct S3Command {
    pub s3: String,
    pub target: Option<String>,
    /// `None` when no operation was named, which the run refuses with examples.
    pub operation: Option<S3Operation>,
    pub search: Option<String>,
    pub recursive: bool,
    pub cursor: Option<String>,
    pub max_objects: Option<u64>,
    pub region: Option<String>,
    pub dry_run: bool,
    pub json: bool,
    pub bytes: Option<u64>,
    pub offset_bytes: Option<u64>,
    pub if_match: Option<String>,
}

impl S3Command {
    pub fn parse(matches: &ArgMatches) -> Self {
        let content = matches.get_one::<String>("search-content").cloned();
        let search = matches.get_one::<String>("search").cloned();
        let operation = if matches.get_flag("buckets") {
            Some(S3Operation::Buckets)
        } else if matches.get_flag("list") {
            Some(S3Operation::List)
        } else if matches.get_flag("head") {
            Some(S3Operation::Head)
        } else if matches.get_flag("preview") {
            Some(S3Operation::Preview)
        } else if content.is_some() {
            Some(S3Operation::ContentSearch)
        } else {
            search.as_ref().map(|_| S3Operation::Search)
        };
        let search = search.or(content);
        Self {
            s3: matches
                .get_one::<String>("s3")
                .expect("clap requires S3")
                .clone(),
            target: matches.get_one::<String>("target").cloned(),
            operation,
            search,
            recursive: matches.get_flag("recursive"),
            cursor: matches.get_one::<String>("cursor").cloned(),
            max_objects: matches.get_one::<u64>("max-objects").copied(),
            region: matches.get_one::<String>("region").cloned(),
            dry_run: matches.get_flag("dry-run"),
            json: matches.get_flag("json"),
            bytes: matches.get_one::<u64>("bytes").copied(),
            offset_bytes: matches.get_one::<u64>("offset-bytes").copied(),
            if_match: matches.get_one::<String>("if-match").cloned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<S3Command, clap::Error> {
        let matches =
            command().try_get_matches_from(std::iter::once("s3").chain(args.iter().copied()))?;
        Ok(S3Command::parse(&matches))
    }

    #[test]
    fn s3_command_takes_one_operation() {
        let command = parse(&[
            "assets",
            "s3://b/p/",
            "--search",
            "inv",
            "--max-objects",
            "5",
        ])
        .unwrap();
        assert_eq!(command.operation, Some(S3Operation::Search));
        assert_eq!(command.search.as_deref(), Some("inv"));
        assert_eq!(command.max_objects, Some(5));
        assert_eq!(parse(&["assets"]).unwrap().operation, None);
        assert_eq!(
            parse(&["assets", "--list", "--recursive"])
                .unwrap()
                .operation,
            Some(S3Operation::List)
        );
        let preview = parse(&[
            "assets",
            "s3://b/k.json",
            "--preview",
            "--bytes",
            "1048576",
            "--offset-bytes",
            "7",
            "--if-match",
            "\"e\"",
        ])
        .unwrap();
        assert_eq!(preview.operation, Some(S3Operation::Preview));
        assert_eq!(
            (
                preview.bytes,
                preview.offset_bytes,
                preview.if_match.as_deref()
            ),
            (Some(1_048_576), Some(7), Some("\"e\""))
        );
        let content = parse(&["assets", "--search-content", "id-1"]).unwrap();
        assert_eq!(content.operation, Some(S3Operation::ContentSearch));
        assert_eq!(content.search.as_deref(), Some("id-1"));
        assert_eq!(
            parse(&["assets", "s3://b/k", "--head"]).unwrap().operation,
            Some(S3Operation::Head)
        );
        for refused in [
            &["assets", "--list", "--buckets"][..],
            &["assets", "--list", "--search", "x"],
            &["assets", "--recursive"],
            &["assets", "--search", "x", "--recursive"],
            &["assets", "s3://b/", "--buckets"],
            &["assets", "--buckets", "--cursor", "c"],
            &["assets", "--list", "--max-objects", "0"],
            &["assets", "--list", "--max-objects", "100001"],
            &["assets", "--head", "--preview"],
            &["assets", "--search", "x", "--search-content", "y"],
            &["assets", "--head", "--cursor", "c"],
            &["assets", "--preview", "--max-objects", "5"],
            &["assets", "--head", "--bytes", "10"],
            &["assets", "--preview", "--bytes", "0"],
            &["assets", "--preview", "--bytes", "1048577"],
            &["assets", "--search-content", "x", "--recursive"],
            &["assets", "--list", "--if-match", "e"],
        ] {
            assert!(parse(refused).is_err(), "{refused:?}");
        }
    }
}
