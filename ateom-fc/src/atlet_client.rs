use crate::atelet::ateom_support_client::AteomSupportClient;
use crate::atelet::{AttributeEntry, Limits, RegisterWorkerRequest, Resources, SandboxRuntime, VersionedSandboxCompat, WorkerResources};
use anyhow::Context;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tonic_tls::Transport;

// didnt know how to do mtls, so asked claude to write it.
// TODO: for sgavulla, review this code and rewrite it
const ATLET_UDS_PATH: &str = "/var/lib/ate/ateom-support.sock";
const ATELET_SPIFFE_ID: &str = "spiffe://cluster.local/ns/ate-system/sa/atelet";
const CREDENTIAL_BUNDLE_PATH: &str = "/run/podidentity.podcert.ate.dev/credential-bundle.pem";
const TRUST_BUNDLE_PATH: &str = "/run/podidentity.podcert.ate.dev/trust-bundle.pem";

// Where atecontroller projects this pod's own CPU/memory limits via the
// Kubernetes downward API (see ateom.CapacityMountPath on the Go side).
const CAPACITY_MOUNT_PATH: &str = "/run/ateom-capacity";

const INITIAL_REPORT_BACKOFF: Duration = Duration::from_millis(500);
const MAX_REPORT_BACKOFF: Duration = Duration::from_secs(30);

// The Substrate-specific X.509 extension OID that carries a JSON-encoded
// PodIdentity (see substratex509.go: GoogleSubstratePEN + sub-arc 1).
const POD_IDENTITY_OID: &[u64] = &[1, 3, 6, 1, 4, 1, 11129, 2, 12, 1];

struct Credentials {
    private_key: PrivateKeyDer<'static>,
    worker_cert_chain: Vec<CertificateDer<'static>>,
    trust_bundle_certs: Vec<CertificateDer<'static>>,
}
pub struct AtletClient {}

impl AtletClient {
    pub fn new() -> Self {
        Self {}
    }

    // atlet and the pod use strict mtls , so we need to set it up before we can connect to atlet.
    pub async fn connect(&self) -> anyhow::Result<()> {
        let credentials = Self::read_credentials()?;
        let my_identity = Self::pod_identity_from_cert(&credentials.worker_cert_chain[0])?;
        let verifier = Self::build_server_cert_verifier(my_identity, credentials.trust_bundle_certs.clone());
        let tls_config = Self::build_client_config(&credentials, verifier)?;

        let channel = Self::build_tonic_channel(tls_config).await?;
        let client = AteomSupportClient::new(channel);

        Self::register_worker(client).await?;
        Ok(())
    }

    // Step 1 (done): read the private pod cert and the trust bundle.
    fn read_credentials() -> anyhow::Result<Credentials> {
        let private_pod_cert = fs::read(CREDENTIAL_BUNDLE_PATH)?;
        let trust_bundle = fs::read(TRUST_BUNDLE_PATH)?;

        let mut private_key = rustls_pemfile::pkcs8_private_keys(&mut private_pod_cert.as_slice())
            .collect::<Result<Vec<_>, _>>()?;
        let worker_cert_chain = rustls_pemfile::certs(&mut private_pod_cert.as_slice())
            .collect::<Result<Vec<_>, _>>()?;
        let trust_bundle_certs = rustls_pemfile::certs(&mut trust_bundle.as_slice())
            .collect::<Result<Vec<_>, _>>()?;

        anyhow::ensure!(private_key.len() == 1, "expected exactly one private key in credential bundle, found {}", private_key.len());
        anyhow::ensure!(!worker_cert_chain.is_empty(), "credential bundle has no certificates");
        anyhow::ensure!(!trust_bundle_certs.is_empty(), "trust bundle has no certificates");

        Ok(Credentials {
            private_key: private_key.remove(0).into(),
            worker_cert_chain,
            trust_bundle_certs,
        })
    }

    // Step 2a: reads the Substrate-specific PodIdentity X.509 extension (OID
    // 1.3.6.1.4.1.11129.2.12.1, a JSON blob -- see substratex509.go) out of a
    // certificate, to learn its namespace/pod/node identity. Used both on our
    // own leaf cert (to learn our node) and on atelet's presented cert (to
    // check it claims the same node).
    fn pod_identity_from_cert(cert: &CertificateDer<'_>) -> anyhow::Result<PodIdentity> {
        let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
            .map_err(|e| anyhow::anyhow!("parsing certificate: {e}"))?;
        let oid = x509_parser::oid_registry::Oid::from(POD_IDENTITY_OID)
            .map_err(|_| anyhow::anyhow!("building PodIdentity OID"))?;
        let ext = parsed
            .get_extension_unique(&oid)
            .map_err(|_| anyhow::anyhow!("certificate contains multiple PodIdentity extensions"))?
            .context("certificate has no PodIdentity extension")?;
        serde_json::from_slice(ext.value).context("decoding PodIdentity extension")
    }

    // Step 2b: the custom verifier -- checks atelet's cert carries the right
    // SPIFFE URI (in its SAN) *and* the same NodeName/NodeUID as our own cert
    // (via pod_identity_from_cert), instead of a normal hostname check.
    fn build_server_cert_verifier(
        my_identity: PodIdentity,
        trust_bundle_certs: Vec<CertificateDer<'static>>,
    ) -> Arc<dyn ServerCertVerifier> {
        Arc::new(AteletCertVerifier {
            my_identity,
            trust_bundle_certs,
            provider: rustls::crypto::CryptoProvider::get_default()
                .cloned()
                .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider())),
        })
    }

    // Step 3: assembles the rustls ClientConfig -- our own client certificate
    // (so atelet can verify us) plus the custom verifier from step 2b (so we
    // verify atelet).
    fn build_client_config(
        credentials: &Credentials,
        verifier: Arc<dyn ServerCertVerifier>,
    ) -> anyhow::Result<rustls::ClientConfig> {
        let mut config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(credentials.worker_cert_chain.clone(), credentials.private_key.clone_key())
            .context("building rustls client config")?;
        // gRPC is HTTP/2-only; without ALPN advertising "h2", the TLS
        // handshake succeeds but the HTTP/2 layer on top of it can break
        // (observed as a broken-pipe transport error on the first RPC).
        config.alpn_protocols = vec![tonic_tls::ALPN_H2.to_vec()];
        Ok(config)
    }

    // Step 4, 5 + 6: builds a tonic Channel backed by a redialable connector
    // (tonic_tls::rustls::TlsConnector) instead of a single pre-connected
    // stream -- tonic's Channel reconnects on transport errors (e.g. a
    // dropped connection) by calling the connector again, which a one-shot
    // connector can't support (it would panic on the second call). UdsTransport
    // below supplies a fresh raw Unix socket connection each time the
    // connector needs one; tonic_tls wraps each one in a fresh TLS handshake
    // (where the verifier from step 2b runs) automatically.
    async fn build_tonic_channel(tls_config: rustls::ClientConfig) -> anyhow::Result<Channel> {
        // The custom verifier above ignores the server name (it checks SPIFFE
        // identity + node pinning instead), so this is a fixed placeholder,
        // not a real hostname.
        let domain = ServerName::try_from("ateom-support").context("building placeholder TLS server name")?;
        let transport = UdsTransport { path: ATLET_UDS_PATH };
        let connector = tonic_tls::rustls::TlsConnector::new(transport, Arc::new(tls_config), domain);

        let channel = Endpoint::try_from("http://ateom-support")
            .context("building placeholder tonic endpoint")?
            .connect_with_connector(connector)
            .await
            .context("building tonic channel over the atelet TLS connector")?;
        Ok(channel)
    }

    // Step 7 + 8: sends the RegisterWorkerRequest (actors: 1, our CPU/memory
    // limits, sandbox_class: "firecracker"), retrying with backoff until it
    // succeeds -- the Worker record may not exist in ateapi yet the instant
    // this pod starts.
    async fn register_worker(mut client: AteomSupportClient<Channel>) -> anyhow::Result<()> {
        let request = RegisterWorkerRequest {
            capacity: Some(Self::read_capacity()),
            default_runtime: Some(SandboxRuntime {
                sandbox_class: "firecracker".to_string(),
                // Mirrors hardware.ProbeHost() on the Go side, which today
                // only reports one attribute: the CPU architecture. cpu_features
                // etc. are TODOs over there too, not just here.
                version: Some(VersionedSandboxCompat {
                    schema_version: "v1".to_string(),
                    attributes: vec![AttributeEntry {
                        key: "architecture".to_string(),
                        value: Self::go_arch().to_string(),
                    }],
                }),
            }),
            restorable_runtimes: vec![],
        };

        let mut backoff = INITIAL_REPORT_BACKOFF;
        loop {
            match client.register_worker(request.clone()).await {
                Ok(_) => {
                    println!("registered worker capacity with atelet");
                    return Ok(());
                }
                Err(status) => {
                    println!("retrying worker capacity report in {backoff:?}: {status}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_REPORT_BACKOFF);
                }
            }
        }
    }

    // Reads this pod's own CPU/memory limits from the downward-API files
    // atecontroller projects in (see CAPACITY_MOUNT_PATH), pairing them with
    // a flat actor-slot count. One Firecracker microVM per pod for now, so 1
    // is the right ceiling -- see the discussion on gvisor/microvm's
    // --max-actors defaulting to 1000 but never actually binding, since the
    // resource dimension below always saturates first for a VM-sized actor.
    fn read_capacity() -> WorkerResources {
        let cpu_milli = Self::read_capacity_file("cpu_milli");
        let memory_bytes = Self::read_capacity_file("memory_bytes");

        let mut limits = Vec::new();
        if cpu_milli != 0 {
            limits.push(Limits { name: "cpu".to_string(), quantity: format!("{cpu_milli}m") });
        }
        if memory_bytes != 0 {
            limits.push(Limits { name: "memory".to_string(), quantity: memory_bytes.to_string() });
        }

        WorkerResources {
            actors: 1,
            resources: if limits.is_empty() { None } else { Some(Resources { limits }) },
        }
    }

    // A limit that is missing or unparseable is reported as zero, which the
    // control plane reads as none: better to place nothing on a worker that
    // cannot say what it has than to invent a number for it (matches
    // ateom.readLimit on the Go side).
    fn read_capacity_file(name: &str) -> i64 {
        fs::read_to_string(format!("{CAPACITY_MOUNT_PATH}/{name}"))
            .ok()
            .and_then(|contents| contents.trim().parse().ok())
            .unwrap_or(0)
    }

    // Go's runtime.GOARCH (what the rest of Substrate stamps compatibility
    // attributes with) names architectures differently from Rust's own
    // std::env::consts::ARCH ("aarch64" vs "arm64", etc.) -- MatchesCompat on
    // the control-plane side requires exact string equality, so this maps to
    // Go's naming rather than leaking Rust's.
    fn go_arch() -> &'static str {
        match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        }
    }
}

// Dials the fixed atelet UDS path fresh on every call -- the Uri argument is
// unused, since there's only ever one endpoint (mirrors ateletdial.go's
// grpc.WithContextDialer, which also ignores its passed-in address and
// always dials the one configured socket path).
#[derive(Clone)]
struct UdsTransport {
    path: &'static str,
}

impl Transport for UdsTransport {
    type Io = UnixStream;
    type Error = std::io::Error;

    async fn connect(&self, _uri: &Uri) -> Result<Self::Io, Self::Error> {
        UnixStream::connect(self.path).await
    }
}

#[derive(Debug)]
struct AteletCertVerifier {
    my_identity: PodIdentity,
    trust_bundle_certs: Vec<CertificateDer<'static>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for AteletCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // Chain + server-auth EKU validation against our trust bundle, with
        // no hostname/SAN matching -- atelet's cert has no DNS SAN, only the
        // SPIFFE URI SAN checked explicitly below (mirrors ateletdial.go's
        // InsecureSkipVerify + manual VerifyConnection on the Go side).
        let trust_anchors: Vec<_> = self
            .trust_bundle_certs
            .iter()
            .map(|cert| webpki::anchor_from_trusted_cert(cert).map_err(|e| rustls::Error::General(e.to_string())))
            .collect::<Result<_, _>>()?;
        let ee_cert = webpki::EndEntityCert::try_from(end_entity).map_err(|e| rustls::Error::General(e.to_string()))?;
        ee_cert
            .verify_for_usage(
                self.provider.signature_verification_algorithms.all,
                &trust_anchors,
                intermediates,
                now,
                webpki::KeyUsage::server_auth(),
                None,
                None,
            )
            .map_err(|e| rustls::Error::General(e.to_string()))?;

        // Beyond the chain: atelet's exact SPIFFE ID, which a normal hostname
        // check cannot express (see ateletdial.go's VerifyConnection).
        let (_, cert) = x509_parser::parse_x509_certificate(end_entity.as_ref())
            .map_err(|e| rustls::Error::General(format!("parsing atelet certificate: {e}")))?;

        let spiffe_uri = cert
            .subject_alternative_name()
            .ok()
            .flatten()
            .and_then(|san| {
                san.value.general_names.iter().find_map(|name| match name {
                    x509_parser::extensions::GeneralName::URI(uri) => Some(uri.to_string()),
                    _ => None,
                })
            })
            .ok_or_else(|| rustls::Error::General("atelet certificate has no URI SAN".into()))?;
        if spiffe_uri != ATELET_SPIFFE_ID {
            return Err(rustls::Error::General(format!("node-local peer is not atelet (SPIFFE id {spiffe_uri})")));
        }

        // And: atelet must be on this exact node, not just any atelet in the
        // cluster -- this socket is node-local by design.
        let peer_identity = AtletClient::pod_identity_from_cert(end_entity)
            .map_err(|e| rustls::Error::General(e.to_string()))?;
        if peer_identity.node_name != self.my_identity.node_name || peer_identity.node_uid != self.my_identity.node_uid {
            return Err(rustls::Error::General(format!(
                "atelet is not on worker node {:?} ({})",
                self.my_identity.node_name, self.my_identity.node_uid
            )));
        }

        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

// Mirrors substratex509.PodIdentity (Go): the custom X.509 extension embedded
// by the kubelet's PodCertificate signer, JSON-encoded (not ASN.1) as the
// extension's raw value.
#[derive(Debug, Clone, serde::Deserialize)]
struct PodIdentity {
    #[serde(rename = "Namespace")]
    namespace: String,
    #[serde(rename = "ServiceAccountName")]
    service_account_name: String,
    #[serde(rename = "ServiceAccountUID")]
    service_account_uid: String,
    #[serde(rename = "PodName")]
    pod_name: String,
    #[serde(rename = "PodUID")]
    pod_uid: String,
    #[serde(rename = "NodeName")]
    node_name: String,
    #[serde(rename = "NodeUID")]
    node_uid: String,
}
