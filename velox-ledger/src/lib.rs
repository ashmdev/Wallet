pub mod config;
pub mod domain;
pub mod error;
pub mod repository;
pub mod service;
pub mod webhook;

pub mod proto {
    tonic::include_proto!("veloxledger.v1");
}
