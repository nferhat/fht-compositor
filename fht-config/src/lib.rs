#![feature(sync_unsafe_cell)]
use std::cell::SyncUnsafeCell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::ops::Deref;
use std::path::PathBuf;

use ron::extensions::Extensions;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub trait Config: Clone + Default + Serialize + DeserializeOwned {
    const NAME: &'static str;

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

#[derive(Debug)]
pub struct ConfigWrapper<Inner: Config + Sized> {
    inner: SyncUnsafeCell<Option<Inner>>,
}

impl<Inner: Config> ConfigWrapper<Inner> {
    pub const fn new() -> Self {
        Self {
            inner: SyncUnsafeCell::new(None),
        }
    }

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

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("I/O while loading the config: {0:?}")]
    Io(#[from] std::io::Error),
    #[error("Error while parsing the config: {0:?}")]
    Parse(#[from] ron::Error),
}
