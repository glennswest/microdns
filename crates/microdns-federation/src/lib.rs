pub mod coordinator;
pub mod heartbeat;
pub mod leaf;
pub mod replication;
pub mod sync;

pub(crate) mod proto {
    tonic::include_proto!("microdns");
}
