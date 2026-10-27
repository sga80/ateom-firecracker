use crate::ateom::{CheckpointWorkloadRequest, CheckpointWorkloadResponse, GetActiveWorkloadStatsRequest, GetActiveWorkloadStatsResponse, GetWorkloadStatsRequest, GetWorkloadStatsResponse, RestoreWorkloadRequest, RestoreWorkloadResponse, RunWorkloadRequest, RunWorkloadResponse, TerminateWorkloadRequest, TerminateWorkloadResponse};
use crate::{ateom, booter, docker_ext4, fcstarter, tap, virtual_sock};
use anyhow::Context;
use std::fs;
use std::path::Path;
use tokio::time::Instant;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{Request, Response, Status};

const BASE_UDS_PATH: &str = "/var/lib/ate/ateoms";
pub(crate) struct AteomService {
    pod_uuid: String,
}

impl AteomService {
    pub fn new(pod_uuid: String) -> Self {
        AteomService { pod_uuid }
    }
    pub async fn start_server(&self) -> anyhow::Result<(UnixListenerStream)> {
        let socket_path = format!("{}/{}/{}", BASE_UDS_PATH, self.pod_uuid, "ateom.sock");
        let parent = Path::new(&socket_path).parent().context("socket path doesnt have a parent")?;
        fs::create_dir_all(parent)?;
        println!("created uds socket parent dir");
        let _ = fs::remove_file(&socket_path);
        let listener = tokio::net::UnixListener::bind(socket_path)?;
        println!("started listener to socker path");
        let stream = tokio_stream::wrappers::UnixListenerStream::new(listener);
        println!("starting server");

        Ok((stream))
    }

    pub async fn start_incoming(self, stream: UnixListenerStream) -> anyhow::Result<()> {
        tonic::transport::Server::builder()
            .add_service(ateom::ateom_server::AteomServer::new(self))
            .serve_with_incoming(stream)
            .await?;
        Ok(())
    }
}
#[tonic::async_trait]
impl ateom::ateom_server::Ateom for AteomService {
    async fn run_workload(&self, request: Request<RunWorkloadRequest>) -> Result<Response<RunWorkloadResponse>, Status> {
        let request = request.into_inner();
        println!("received request: {:?}", request);
        if let Some(actor_dirs) = request.actor_dirs {
            let actor_uid = request.actor_uid;

            // Holds one OCI bundle per container, named after the container.
            // We only support the first container for now.
            let container_name = request
                .spec
                .as_ref()
                .and_then(|spec| spec.containers.first())
                .map(|container| container.name.as_str())
                .context("run workload request has no containers")
                .map_err(|e| Status::internal(e.to_string()))?;

            let bundle_dir = Path::new(&actor_dirs.oci_bundle_dir).join(container_name);
            let rootfs_overlay_json = bundle_dir.join("rootfs-overlay.json");
            let rootfs_overlay_json = rootfs_overlay_json
                .to_str()
                .context("rootfs-overlay.json path is not valid UTF-8")
                .map_err(|e| Status::internal(e.to_string()))?;

            let (composed_rootfs_path, image_digest) = docker_ext4::setup_bundle_rootfs(rootfs_overlay_json)
                .map_err(|e| Status::internal(e.to_string()))?;
            let docker_image_ext4_path = docker_ext4::docker_lower_ext4_block_device(&composed_rootfs_path, &image_digest)
                .map_err(|e| Status::internal(e.to_string()))?;
            let docker_image_rw_upper_path = docker_ext4::docker_upper_ext4_block_device(actor_uid.clone())
                .map_err(|e| Status::internal(e.to_string()))?;

            let client = get_reqwest_http_client().map_err(|e| Status::internal(e.to_string()))?;
            // // hardcoding the IP address for now. When we implement IPAM we will be unique IP address for each VM
            tap::setup_actor_tap(&actor_uid, "169.254.17.1".to_string())
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
            let fc_start_time = Instant::now();
            // start fircracker
            fcstarter::start(client.clone(), &actor_uid)
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
            println!("firecracker started, took {:?}", fc_start_time.elapsed());


            let kernel_path = "/var/lib/ateomfc/vmlinux";
            let root_fs_path = "/var/lib/ateomfc/rootfs.ext4";
            let mut booter = booter::Booter::new(client.clone(), String::from(kernel_path), String::from(root_fs_path), docker_image_ext4_path, docker_image_rw_upper_path, actor_uid.clone());

            let boot_time = Instant::now();
            booter.boot().await.map_err(|e| Status::internal(e.to_string()))?;
            println!("booter started, boot took {:?}, starting virtual sock now ", boot_time.elapsed());
            // now connect to the af_sock async
            virtual_sock::start_connect(booter.get_uds_path())
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        }


        Ok(Response::new(RunWorkloadResponse {}))
    }

    async fn checkpoint_workload(&self, request: Request<CheckpointWorkloadRequest>) -> Result<Response<CheckpointWorkloadResponse>, Status> {
        todo!()
    }

    async fn restore_workload(&self, request: Request<RestoreWorkloadRequest>) -> Result<Response<RestoreWorkloadResponse>, Status> {
        todo!()
    }

    async fn get_workload_stats(&self, request: Request<GetWorkloadStatsRequest>) -> Result<Response<GetWorkloadStatsResponse>, Status> {
        todo!()
    }

    async fn get_active_workload_stats(&self, request: Request<GetActiveWorkloadStatsRequest>) -> Result<Response<GetActiveWorkloadStatsResponse>, Status> {
        todo!()
    }

    async fn terminate_workload(&self, request: Request<TerminateWorkloadRequest>) -> Result<Response<TerminateWorkloadResponse>, Status> {
        todo!()
    }
}
fn get_reqwest_http_client() -> anyhow::Result<reqwest::Client> {
    let client = reqwest::Client::builder()
        .unix_socket(fcstarter::SOCKET_PATH)
        .build()?;
    Ok(client)
}