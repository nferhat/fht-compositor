#![feature(sync_unsafe_cell)]
use std::cell::SyncUnsafeCell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::ops::Deref;
use std::path::PathBuf;

use ron::extensions::Extensions;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Shared trait for every configuration of fht-shell.
///
/// Name will be used to get the path of the config, as in the following format str:
/// `$XDG_CONFIG_HOME/.config/fht/{Config::NAME}.ron` as the path for the configuration file to
/// load.
pub trait Config: Clone + Default + Serialize + DeserializeOwned {
    /// The name of this config.
    const NAME: &'static str;

    /// The default config file contents to use.
    ///
    /// These will be used to popular a default config file if the file was not found before.
    const DEFAULT_CONTENTS: &'static str = "";

    fn get_path() -> PathBuf {
        xdg::BaseDirectories::new()
            .unwrap()
            .get_config_file(format!("fht/{}.ron", Self::NAME))
    }

    fn load() -> Result<Self, Error> {
        let config_path = Self::get_path();
        let config_path = config_path
            .to_str()
            .expect("Configuration path contains non UTF-8 characters!");

        let reader = OpenOptions::new().read(true).write(false).open(config_path);
        let reader = match reader {
            Ok(reader) => reader,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Create config file for user
                let mut file = File::create_new(config_path).unwrap();
                writeln!(&mut file, "{}", Self::DEFAULT_CONTENTS).unwrap();
                OpenOptions::new()
                    .read(true)
                    .write(false)
                    .open(config_path)
                    .map_err(Error::Io)?
            }
            Err(err) => {
                return Err(Error::Io(err));
            }
        };

        ron::Options::default()
            .with_default_extension(Extensions::IMPLICIT_SOME)
            .with_default_extension(Extensions::UNWRAP_VARIANT_NEWTYPES)
            .from_reader(reader)
            .map_err(|err| Error::Parse(err.code))
    }
}

/// A config wrapper to use any config struct statically.
///
/// This type will be able to load
#[derive(Debug)]
pub struct ConfigWrapper<Inner: Config + Sized> {
    inner: SyncUnsafeCell<Option<Inner>>,
}

impl<Inner: Config> ConfigWrapper<Inner> {
    /// Creates a new uninitialized configuration.
    ///
    /// SAFETY: Its up to YOU to initialize the config.
    pub const fn new() -> Self {
        Self {
            inner: SyncUnsafeCell::new(None),
        }
    }

    /// Get a reference to the inner unsafe cell.
    pub fn get(&self) -> &Inner {
        let inner_ref = unsafe {
            self.inner
                .get()
                .as_ref()
                .expect("Configuration points to NULL!")
                .as_ref()
        };
        inner_ref.expect("Tried to get configuration before initializing it!")
    }

    /// Set the new configuration.
    ///
    /// NOTE: If you want to access the old configuration before updating, you can clone it before
    /// using this.
    ///
    /// # SAFETY
    ///
    /// This function uses an [`SyncUnsafeCell`] internally, and this means that there's no
    /// guarantee for data races, thread safety, etc. It is up to YOU and you ONLY to ensure that
    /// nothing bad happens while setting the config.
    pub fn set(&self, new: Inner) {
        unsafe { *self.inner.get() = Some(new) };
    }
}

// We use an unsafe cell to manage this state.
// Everything should be fine since we don't do much multithreading in the codebase anyway.
unsafe impl<Inner: Config> Send for ConfigWrapper<Inner> {}
unsafe impl<Inner: Config> Sync for ConfigWrapper<Inner> {}

impl<Inner: Config> Deref for ConfigWrapper<Inner> {
    type Target = Inner;

    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

/// Configuration loading error.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O while loading the config: {0:?}")]
    Io(#[from] std::io::Error),
    #[error("Error while parsing the config: {0:?}")]
    Parse(#[from] ron::Error),
}

// TODO: Write tests.
//
// This should be easy since we are using generics to parse everything, but still, have to write
// down and think about which cases we should test here.
