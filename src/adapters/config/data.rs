//! Named file-analysis workspaces and S3 credential references.
use crate::domain::types::{
    dataset::{DataError, DataLimits, DataSource, InvalidInput},
    s3_browse::{MAX_PAGE_SIZE, is_bucket_name},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DataWorkspace {
    pub root_dir: Option<String>,
    pub aws_profile: Option<String>,
    pub s3_source: Option<String>,
    pub region: Option<String>,
    #[serde(default)]
    pub sources: Vec<DataSource>,
    #[serde(flatten)]
    pub limits: DataLimits,
}
impl DataWorkspace {
    pub fn validate(&self) -> Result<(), DataError> {
        self.limits.validate()?;
        if self.sources.is_empty() {
            return Err(InvalidInput::WorkspaceNeedsSource.into());
        }
        if self.aws_profile.is_some() && self.s3_source.is_some() {
            return Err(InvalidInput::CredentialSourceConflict.into());
        }
        if self
            .aws_profile
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(InvalidInput::EmptyAwsProfile.into());
        }
        if self
            .region
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(InvalidInput::EmptyRegion.into());
        }
        if self
            .root_dir
            .as_ref()
            .is_some_and(|p| !Path::new(p).is_absolute())
        {
            return Err(InvalidInput::RootDirMustBeAbsolute.into());
        }
        let mut names = BTreeSet::new();
        for source in &self.sources {
            if source.name.is_empty() || !names.insert(source.name.to_lowercase()) {
                return Err(InvalidInput::InvalidSourceNames.into());
            }
            if source.path.is_empty() || source.path.contains('\0') {
                return Err(InvalidInput::InvalidSourcePath.into());
            }
            source.inferred_format()?;
            if !source.path.starts_with("s3://")
                && !Path::new(&source.path).is_absolute()
                && self.root_dir.is_none()
            {
                return Err(InvalidInput::RelativeSourceNeedsRoot.into());
            }
            if source.path.contains("://") && !source.path.starts_with("s3://") {
                return Err(InvalidInput::UnsupportedSourceProtocol.into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct S3Connection {
    pub aws_profile: String,
    pub region: Option<String>,
    /// Where `kurama s3` starts; not a permission boundary.
    pub bucket: Option<String>,
    pub prefix: Option<String>,
    /// Keys a `ListObjectsV2` page asks for (1-1000).
    pub page_size: Option<u32>,
    pub request_timeout_secs: Option<u64>,
}

impl S3Connection {
    pub fn validate(&self) -> Result<(), DataError> {
        if self.aws_profile.trim().is_empty() {
            return Err(InvalidInput::EmptyAwsProfile.into());
        }
        if self
            .region
            .as_ref()
            .is_some_and(|region| region.trim().is_empty())
        {
            return Err(InvalidInput::EmptyRegion.into());
        }
        if self
            .bucket
            .as_ref()
            .is_some_and(|bucket| !is_bucket_name(bucket))
        {
            return Err(InvalidInput::S3BucketName.into());
        }
        if self.prefix.is_some() && self.bucket.is_none() {
            return Err(InvalidInput::S3PrefixNeedsBucket.into());
        }
        if self
            .page_size
            .is_some_and(|size| !(1..=MAX_PAGE_SIZE).contains(&size))
        {
            return Err(InvalidInput::S3PageSize.into());
        }
        if self.request_timeout_secs == Some(0) {
            return Err(InvalidInput::S3RequestTimeout.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::adapters::config::Config;
    #[test]
    fn data_config_validates_workspace_and_limits() {
        let valid = "[data.lake]\nroot_dir='/tmp'\nthreads=2\n[[data.lake.sources]]\nname='orders'\npath='orders.csv'\ntypes={amount='DECIMAL(18,2)'}\n";
        assert!(Config::parse(valid).is_ok());
        for invalid in [
            valid.replace("threads=2", "threads=0"),
            valid.replace("threads=2", "region=' '"),
            valid.replace("threads=2", "aws_profile=''"),
            valid.replace("threads=2", "unknown=2"),
            valid.replace("root_dir='/tmp'", "root_dir='relative'"),
            format!("{valid}[[data.lake.sources]]\nname='ORDERS'\npath='two.csv'\n"),
        ] {
            assert!(Config::parse(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn s3_config_bounds_the_browse_start_and_its_pages() {
        let valid = "[s3.assets]\naws_profile='dev'\nbucket='my.assets-1'\nprefix='日本語 //'\npage_size=1000\nrequest_timeout_secs=1\n";
        assert!(Config::parse(valid).is_ok());
        for (from, to, message) in [
            ("bucket='my.assets-1'", "bucket='a/b'", "bucket must be"),
            ("bucket='my.assets-1'\n", "", "prefix needs a bucket"),
            ("page_size=1000", "page_size=1001", "page_size"),
            ("page_size=1000", "page_size=0", "page_size"),
            (
                "request_timeout_secs=1",
                "request_timeout_secs=0",
                "request_timeout_secs",
            ),
        ] {
            let error = Config::parse(&valid.replace(from, to)).unwrap_err();
            assert!(error.to_string().contains(message), "{to}: {error}");
        }
    }

    #[test]
    fn data_s3_config_rejects_empty_credentials_and_regions_by_name() {
        for value in [
            "aws_profile=''",
            "aws_profile='dev'\nregion=''",
            "aws_profile='dev'\nregion='   '",
        ] {
            let error = Config::parse(&format!("[s3.assets]\n{value}\n")).unwrap_err();
            assert!(error.to_string().contains("[s3.assets]"), "{error}");
        }
    }
}
