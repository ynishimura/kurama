//! The `[s3.*]` rows of `kurama status` and the home screen's S3 tab, from the configuration alone: nothing is assumed, listed or reached.
use crate::adapters::config::Config;

/// One `[s3.*]` as configured; whether its role or bucket answers is not
/// checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3StatusRow {
    pub name: String,
    pub aws_profile: String,
    pub region: Option<String>,
    pub bucket: Option<String>,
    pub prefix: Option<String>,
}

impl S3StatusRow {
    /// Where the explorer starts: `s3://bucket/prefix`, or `None` when it
    /// starts from the bucket list.
    pub fn start(&self) -> Option<String> {
        self.bucket
            .as_ref()
            .map(|bucket| format!("s3://{bucket}/{}", self.prefix.as_deref().unwrap_or("")))
    }
}

pub fn status_rows(config: &Config, name: Option<&str>) -> Vec<S3StatusRow> {
    config
        .s3
        .iter()
        .filter(|(s3, _)| name.is_none_or(|name| *s3 == name))
        .map(|(name, connection)| S3StatusRow {
            name: name.clone(),
            aws_profile: connection.aws_profile.clone(),
            region: connection.region.clone(),
            bucket: connection.bucket.clone(),
            prefix: connection.prefix.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_status_rows_come_from_the_configuration_alone() {
        let config = Config::parse(
            "[s3.assets]\naws_profile = \"dev\"\nbucket = \"b\"\nprefix = \"p/\"\n\n[s3.any]\naws_profile = \"ops\"\nregion = \"us-west-2\"\n",
        )
        .unwrap();
        let rows = status_rows(&config, None);
        assert_eq!(rows.len(), 2);
        let assets = rows.iter().find(|row| row.name == "assets").unwrap();
        assert_eq!(assets.start().as_deref(), Some("s3://b/p/"));
        let any = rows.iter().find(|row| row.name == "any").unwrap();
        assert_eq!(
            (any.start(), any.region.as_deref()),
            (None, Some("us-west-2"))
        );
        assert_eq!(status_rows(&config, Some("any")), vec![any.clone()]);
    }
}
