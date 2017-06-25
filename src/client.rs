//! Clients for high level interactions with TUF repositories.

use Result;
use crypto;
use error::Error;
use interchange::DataInterchange;
use metadata::{MetadataVersion, RootMetadata, Role, MetadataPath};
use repository::Repository;
use tuf::Tuf;

/// A client that interacts with TUF repositories.
pub struct Client<D, L, R>
where
    D: DataInterchange,
    L: Repository<D>,
    R: Repository<D>,
{
    tuf: Tuf<D>,
    config: Config,
    local: L,
    remote: R,
}

impl<D, L, R> Client<D, L, R>
where
    D: DataInterchange,
    L: Repository<D>,
    R: Repository<D>,
{
    /// Create a new TUF client from the given `Tuf` (metadata storage) and local and remote
    /// repositories.
    pub fn new(tuf: Tuf<D>, config: Config, local: L, remote: R) -> Self {
        Client {
            tuf: tuf,
            config: config,
            local: local,
            remote: remote,
        }
    }

    /// Update TUF metadata from the local repository.
    ///
    /// Returns `true` if an update occurred and `false` otherwise.
    pub fn update_local(&mut self) -> Result<bool> {
        let r = Self::update_root(&mut self.tuf, &mut self.local, &self.config.max_root_size)?;
        let ts = match Self::update_timestamp(
            &mut self.tuf,
            &mut self.local,
            &self.config.max_timestamp_size,
        ) {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    "Error updating timestamp metadata from local sources: {:?}",
                    e
                );
                false
            }
        };
        let sn = match Self::update_snapshot(&mut self.tuf, &mut self.local) {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    "Error updating snapshot metadata from local sources: {:?}",
                    e
                );
                false
            }
        };
        let ta = match Self::update_targets(&mut self.tuf, &mut self.local) {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    "Error updating targets metadata from local sources: {:?}",
                    e
                );
                false
            }
        };

        Ok(r || ts || sn || ta)
    }

    /// Update TUF metadata from the remote repository.
    ///
    /// Returns `true` if an update occurred and `false` otherwise.
    pub fn update_remote(&mut self) -> Result<bool> {
        Ok(
            Self::update_root(&mut self.tuf, &mut self.remote, &self.config.max_root_size)? ||
                Self::update_timestamp(
                    &mut self.tuf,
                    &mut self.remote,
                    &self.config.max_timestamp_size,
                )? || Self::update_snapshot(&mut self.tuf, &mut self.remote)? ||
                Self::update_targets(&mut self.tuf, &mut self.local)?,
        )
    }

    /// Returns `true` if an update occurred and `false` otherwise.
    fn update_root<T>(tuf: &mut Tuf<D>, repo: &mut T, max_root_size: &Option<usize>) -> Result<bool>
    where
        T: Repository<D>,
    {
        let latest_root = repo.fetch_metadata(
            &Role::Root,
            &MetadataVersion::None,
            max_root_size,
            None,
        )?;
        let latest_version = D::deserialize::<RootMetadata>(latest_root.unverified_signed())?
            .version();

        if latest_version < tuf.root().version() {
            return Err(Error::VerificationFailure(format!(
                "Latest root version is lower than current root version: {} < {}",
                latest_version,
                tuf.root().version()
            )));
        } else if latest_version == tuf.root().version() {
            return Ok(false);
        }

        let err_msg = "TUF claimed no update occurred when one should have. \
                       This is a programming error. Please report this as a bug.";

        for i in (tuf.root().version() + 1)..latest_version {
            let signed = repo.fetch_metadata(
                &Role::Root,
                &MetadataVersion::Number(i),
                max_root_size,
                None,
            )?;
            if !tuf.update_root(signed)? {
                error!("{}", err_msg);
                return Err(Error::Generic(err_msg.into()));
            }
        }

        if !tuf.update_root(latest_root)? {
            error!("{}", err_msg);
            return Err(Error::Generic(err_msg.into()));
        }
        Ok(true)
    }

    /// Returns `true` if an update occurred and `false` otherwise.
    fn update_timestamp<T>(
        tuf: &mut Tuf<D>,
        repo: &mut T,
        max_timestamp_size: &Option<usize>,
    ) -> Result<bool>
    where
        T: Repository<D>,
    {
        let ts = repo.fetch_metadata(
            &Role::Timestamp,
            &MetadataVersion::None,
            max_timestamp_size,
            None,
        )?;
        tuf.update_timestamp(ts)
    }

    /// Returns `true` if an update occurred and `false` otherwise.
    fn update_snapshot<T>(tuf: &mut Tuf<D>, repo: &mut T) -> Result<bool>
    where
        T: Repository<D>,
    {
        let snapshot_description = match tuf.timestamp() {
            Some(ts) => {
                match ts.meta().get(&MetadataPath::from_role(&Role::Timestamp)) {
                    Some(d) => Ok(d),
                    None => Err(Error::VerificationFailure(
                        "Timestamp metadata did not contain a description of the \
                                current snapshot metadata."
                            .into(),
                    )),
                }
            }
            None => Err(Error::MissingMetadata(Role::Timestamp)),
        }?
            .clone();

        let hashes = match snapshot_description.hashes() {
            Some(hashes) => Some(crypto::hash_preference(hashes)?),
            None => None,
        };

        let snap = repo.fetch_metadata(
            &Role::Snapshot,
            &MetadataVersion::None,
            &snapshot_description.length(),
            hashes,
        )?;
        tuf.update_snapshot(snap)
    }

    /// Returns `true` if an update occurred and `false` otherwise.
    fn update_targets<T>(tuf: &mut Tuf<D>, repo: &mut T) -> Result<bool>
    where
        T: Repository<D>,
    {
        let targets_description = match tuf.snapshot() {
            Some(sn) => {
                match sn.meta().get(&MetadataPath::from_role(&Role::Targets)) {
                    Some(d) => Ok(d),
                    None => Err(Error::VerificationFailure(
                        "Snapshot metadata did not contain a description of the \
                                current targets metadata."
                            .into(),
                    )),
                }
            }
            None => Err(Error::MissingMetadata(Role::Snapshot)),
        }?
            .clone();

        let hashes = match targets_description.hashes() {
            Some(hashes) => Some(crypto::hash_preference(hashes)?),
            None => None,
        };

        let targets = repo.fetch_metadata(
            &Role::Targets,
            &MetadataVersion::None,
            &targets_description.length(),
            hashes,
        )?;
        tuf.update_targets(targets)
    }
}

/// Configuration for a TUF `Client`.
#[derive(Debug)]
pub struct Config {
    max_root_size: Option<usize>,
    max_timestamp_size: Option<usize>,
}

impl Config {
    /// Initialize a `ConfigBuilder` with the default values.
    pub fn build() -> ConfigBuilder {
        ConfigBuilder::default()
    }
}

/// Helper for building and validating a TUF `Config`.
#[derive(Debug, PartialEq)]
pub struct ConfigBuilder {
    max_root_size: Option<usize>,
    max_timestamp_size: Option<usize>,
}

impl ConfigBuilder {
    /// Validate this builder return a `Config` if validation succeeds.
    pub fn finish(self) -> Result<Config> {
        Ok(Config {
            max_root_size: self.max_root_size,
            max_timestamp_size: self.max_timestamp_size,
        })
    }

    /// Set the optional maximum download size for root metadata.
    pub fn max_root_size(mut self, max: Option<usize>) -> Self {
        self.max_root_size = max;
        self
    }

    /// Set the optional maximum download size for timestamp metadata.
    pub fn max_timestamp_size(mut self, max: Option<usize>) -> Self {
        self.max_timestamp_size = max;
        self
    }
}

impl Default for ConfigBuilder {
    /// ```
    /// use tuf::client::ConfigBuilder;
    ///
    /// let default = ConfigBuilder::default();
    /// let config = ConfigBuilder::default()
    ///     .max_root_size(Some(1024 * 1024))
    ///     .max_timestamp_size(Some(32 * 1024));
    /// assert_eq!(config, default);
    /// assert!(default.finish().is_ok())
    /// ```
    fn default() -> Self {
        ConfigBuilder {
            max_root_size: Some(1024 * 1024),
            max_timestamp_size: Some(32 * 1024),
        }
    }
}
