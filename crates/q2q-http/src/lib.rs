#![doc = include_str!("../README.md")]
mod checksum;
mod downloader;
mod error;
mod executor;
mod store;

pub use checksum::Checksum;
pub use downloader::{
  DownloadRequest, DownloadState, Downloader, DownloaderBuilder, Progress, Task,
};
pub use error::Error;
pub use executor::{Executor, ExecutorBuilder, Request, Response};
