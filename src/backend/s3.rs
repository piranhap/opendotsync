use aws_sdk_s3::{config::Region, primitives::ByteStream, Client};
use chrono::Utc;
use tokio::runtime::Runtime;

use super::{BackendError, BackendManifest, SnapshotMeta, SnapshotOptions, StorageBackend};

/// Synchronous adapter around the AWS SDK. The owned runtime also keeps the
/// SDK's HTTP connection pool alive between operations.
pub struct S3Backend {
    client: Client,
    bucket: String,
    prefix: String,
    runtime: Runtime,
}

impl S3Backend {
    pub fn new(
        bucket: String,
        region: Option<&str>,
        prefix: Option<&str>,
        endpoint: Option<&str>,
    ) -> Result<Self, BackendError> {
        let runtime = Runtime::new()?;
        let client = runtime.block_on(async {
            let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                .region(Region::new(region.unwrap_or("us-east-1").to_owned()))
                .load()
                .await;
            // Some S3-compatible services do not support the SDK's optional
            // streaming checksum trailers. Retain checksums where required.
            let mut builder = aws_sdk_s3::config::Builder::from(&config)
                .request_checksum_calculation(
                    aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired,
                );
            if let Some(endpoint) = endpoint {
                // Custom services (including local MinIO) need bucket names in
                // the path rather than in DNS hostnames.
                builder = builder.endpoint_url(endpoint).force_path_style(true);
            }
            Client::from_conf(builder.build())
        });
        Ok(Self {
            client,
            bucket,
            prefix: prefix.unwrap_or("").trim_matches('/').to_owned(),
            runtime,
        })
    }

    fn key(&self, profile: &str, filename: &str) -> String {
        if self.prefix.is_empty() {
            format!("{profile}/{filename}")
        } else {
            format!("{}/{profile}/{filename}", self.prefix)
        }
    }

    async fn get_object(&self, key: &str) -> Result<Option<Vec<u8>>, BackendError> {
        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await;
        match result {
            Ok(output) => Ok(Some(
                output
                    .body
                    .collect()
                    .await
                    .map_err(|e| BackendError::S3(format!("read {key}: {e}")))?
                    .into_bytes()
                    .to_vec(),
            )),
            Err(err) if err.as_service_error().is_some_and(|e| e.is_no_such_key()) => Ok(None),
            Err(err) => Err(BackendError::S3(format!("get {key}: {err:?}"))),
        }
    }

    async fn put_object(&self, key: &str, data: Vec<u8>) -> Result<(), BackendError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data))
            .send()
            .await
            .map_err(|e| BackendError::S3(format!("put {key}: {e:?}")))?;
        Ok(())
    }

    async fn load_manifest(&self, profile: &str) -> Result<BackendManifest, BackendError> {
        match self.get_object(&self.key(profile, "manifest.json")).await? {
            Some(data) => {
                serde_json::from_slice(&data).map_err(|e| BackendError::Manifest(e.to_string()))
            }
            None => Ok(BackendManifest::default()),
        }
    }

    async fn save_manifest(
        &self,
        profile: &str,
        manifest: &BackendManifest,
    ) -> Result<(), BackendError> {
        let data = serde_json::to_vec_pretty(manifest)
            .map_err(|e| BackendError::Manifest(e.to_string()))?;
        self.put_object(&self.key(profile, "manifest.json"), data)
            .await
    }
}

impl StorageBackend for S3Backend {
    fn check_connectivity(&self) -> Result<(), BackendError> {
        self.runtime.block_on(async {
            self.client
                .head_bucket()
                .bucket(&self.bucket)
                .send()
                .await
                .map_err(|e| BackendError::S3(format!("check bucket {}: {e:?}", self.bucket)))?;
            Ok(())
        })
    }

    fn push(&self, profile: &str, data: &[u8], message: Option<&str>) -> Result<u64, BackendError> {
        self.push_snapshot(profile, data, message, SnapshotOptions::default(), None)
    }

    fn push_snapshot(
        &self,
        profile: &str,
        data: &[u8],
        message: Option<&str>,
        options: SnapshotOptions,
        expected_version: Option<u64>,
    ) -> Result<u64, BackendError> {
        self.runtime.block_on(async {
            let mut manifest = self.load_manifest(profile).await?;
            manifest.validate_push(expected_version)?;
            let version = manifest.latest_version + 1;
            self.put_object(
                &self.key(profile, &format!("v{version:03}.tar.zst")),
                data.to_vec(),
            )
            .await?;
            manifest.latest_version = version;
            manifest.snapshots.push(SnapshotMeta {
                version,
                timestamp: Utc::now(),
                size_bytes: data.len() as u64,
                message: message.map(str::to_owned),
                snapshot_type: options.snapshot_type,
                base_version: options.base_version,
                deleted_files: options.deleted_files,
            });
            self.save_manifest(profile, &manifest).await?;
            Ok(version)
        })
    }

    fn pull(&self, profile: &str, version: Option<u64>) -> Result<(u64, Vec<u8>), BackendError> {
        self.runtime.block_on(async {
            let manifest = self.load_manifest(profile).await?;
            let version = match version {
                Some(version) => version,
                None if manifest.latest_version == 0 => {
                    return Err(BackendError::NoSnapshots(profile.to_owned()))
                }
                None => manifest.latest_version,
            };
            let data = self
                .get_object(&self.key(profile, &format!("v{version:03}.tar.zst")))
                .await?
                .ok_or(BackendError::VersionNotFound(version))?;
            Ok((version, data))
        })
    }

    fn list_versions(&self, profile: &str) -> Result<Vec<SnapshotMeta>, BackendError> {
        self.runtime
            .block_on(async { Ok(self.load_manifest(profile).await?.snapshots) })
    }

    fn prune(&self, profile: &str, keep: usize) -> Result<(), BackendError> {
        self.runtime.block_on(async {
            let mut manifest = self.load_manifest(profile).await?;
            if manifest.snapshots.len() <= keep {
                return Ok(());
            }
            let remove = manifest.prune_boundary(keep);
            if remove == 0 {
                return Ok(());
            }
            for snapshot in &manifest.snapshots[..remove] {
                let key = self.key(profile, &format!("v{:03}.tar.zst", snapshot.version));
                self.client
                    .delete_object()
                    .bucket(&self.bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|e| BackendError::S3(format!("delete {key}: {e:?}")))?;
            }
            manifest.snapshots.drain(..remove);
            // Preserve the version counter even when keep == 0, like LocalBackend.
            self.save_manifest(profile, &manifest).await
        })
    }

    fn latest_version(&self, profile: &str) -> Result<Option<u64>, BackendError> {
        self.runtime.block_on(async {
            let version = self.load_manifest(profile).await?.latest_version;
            Ok((version != 0).then_some(version))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex,
    };
    use std::thread;
    use std::time::Duration;
    use tiny_http::{Method, Response, Server, StatusCode};

    type Objects = Arc<Mutex<HashMap<String, Vec<u8>>>>;

    struct Store {
        objects: Objects,
        fail: Arc<AtomicU8>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl Store {
        fn start(prefix: Option<&str>) -> (Self, S3Backend) {
            let server = Server::http("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", server.server_addr());
            let objects: Objects = Arc::default();
            let fail = Arc::new(AtomicU8::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let (data, failing, stopping) = (objects.clone(), fail.clone(), stop.clone());
            let worker = thread::spawn(move || {
                while !stopping.load(Ordering::Relaxed) {
                    let Some(mut request) = server.recv_timeout(Duration::from_millis(20)).unwrap()
                    else {
                        continue;
                    };
                    let failure = failing.load(Ordering::Relaxed);
                    if failure == 1
                        || (failure == 2 && request.method() == &Method::Put)
                        || (failure == 3 && request.method() == &Method::Delete)
                    {
                        request.respond(Response::from_string("<Error><Code>AccessDenied</Code><Message>Denied</Message></Error>")
                            .with_status_code(StatusCode(403))).unwrap();
                        continue;
                    }
                    let key = request.url().split('?').next().unwrap().to_owned();
                    let mut objects = data.lock().unwrap();
                    let response = match request.method() {
                        Method::Head => Response::from_data(Vec::new()),
                        Method::Get => match objects.get(&key) {
                            Some(bytes) => Response::from_data(bytes.clone()),
                            None => Response::from_string(
                                "<Error><Code>NoSuchKey</Code><Message>Missing</Message></Error>",
                            )
                            .with_status_code(StatusCode(404)),
                        },
                        Method::Put => {
                            let mut bytes = Vec::new();
                            request.as_reader().read_to_end(&mut bytes).unwrap();
                            objects.insert(key, bytes);
                            Response::from_data(Vec::new())
                        }
                        Method::Delete => {
                            objects.remove(&key);
                            Response::from_data(Vec::new()).with_status_code(StatusCode(204))
                        }
                        method => panic!("unexpected method {method}"),
                    };
                    request.respond(response).unwrap();
                }
            });
            let runtime = Runtime::new().unwrap();
            let client = runtime.block_on(async {
                Client::from_conf(
                    aws_sdk_s3::config::Builder::new()
                        .behavior_version(aws_config::BehaviorVersion::latest())
                        .region(Region::new("us-east-1"))
                        .credentials_provider(aws_sdk_s3::config::Credentials::new(
                            "test", "test", None, None, "test",
                        ))
                        .endpoint_url(endpoint)
                        .force_path_style(true)
                        .request_checksum_calculation(
                            aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired,
                        )
                        .build(),
                )
            });
            (
                Self {
                    objects,
                    fail,
                    stop,
                    worker: Some(worker),
                },
                S3Backend {
                    client,
                    bucket: "dots".into(),
                    prefix: prefix.unwrap_or("").trim_matches('/').into(),
                    runtime,
                },
            )
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.worker.take().unwrap().join().unwrap();
        }
    }

    #[test]
    fn snapshot_lifecycle_and_profile_isolation() {
        let (store, backend) = Store::start(Some("/nested/dots/"));
        backend.check_connectivity().unwrap();
        assert_eq!(backend.latest_version("personal").unwrap(), None);
        assert!(backend.list_versions("personal").unwrap().is_empty());
        assert!(matches!(
            backend.pull("personal", None),
            Err(BackendError::NoSnapshots(_))
        ));
        assert!(matches!(
            backend.pull("personal", Some(9)),
            Err(BackendError::VersionNotFound(9))
        ));
        assert_eq!(backend.push("personal", b"one", Some("first")).unwrap(), 1);
        assert_eq!(backend.push("personal", b"two", None).unwrap(), 2);
        assert_eq!(
            backend.pull("personal", None).unwrap(),
            (2, b"two".to_vec())
        );
        assert_eq!(
            backend.pull("personal", Some(1)).unwrap(),
            (1, b"one".to_vec())
        );
        let versions = backend.list_versions("personal").unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].message.as_deref(), Some("first"));
        assert_eq!(versions[0].size_bytes, 3);
        assert_eq!(backend.latest_version("work").unwrap(), None);
        backend.push("work", b"other", None).unwrap();
        backend.prune("personal", 1).unwrap();
        assert_eq!(backend.list_versions("personal").unwrap().len(), 1);
        assert!(!store
            .objects
            .lock()
            .unwrap()
            .contains_key("/dots/nested/dots/personal/v001.tar.zst"));
        assert!(matches!(
            backend.pull("personal", Some(1)),
            Err(BackendError::VersionNotFound(1))
        ));
        assert_eq!(backend.pull("work", None).unwrap().1, b"other");
        backend.prune("personal", 0).unwrap();
        assert!(backend.list_versions("personal").unwrap().is_empty());
        assert_eq!(backend.push("personal", b"three", None).unwrap(), 3);
    }

    #[test]
    fn errors_are_not_treated_as_an_empty_store() {
        let (store, backend) = Store::start(None);
        store
            .objects
            .lock()
            .unwrap()
            .insert("/dots/test/manifest.json".into(), b"invalid json".to_vec());
        assert!(matches!(
            backend.latest_version("test"),
            Err(BackendError::Manifest(_))
        ));
        store.fail.store(1, Ordering::Relaxed);
        assert!(matches!(
            backend.check_connectivity(),
            Err(BackendError::S3(_))
        ));
        assert!(matches!(
            backend.latest_version("missing"),
            Err(BackendError::S3(_))
        ));
        assert!(matches!(
            backend.push("missing", b"data", None),
            Err(BackendError::S3(_))
        ));
        assert_eq!(store.objects.lock().unwrap().len(), 1);
    }
    #[test]
    fn failed_writes_and_deletes_do_not_publish_a_new_manifest() {
        let (store, backend) = Store::start(None);
        backend.push("test", b"one", None).unwrap();
        backend.push("test", b"two", None).unwrap();
        let manifest = store.objects.lock().unwrap()["/dots/test/manifest.json"].clone();
        store.fail.store(2, Ordering::Relaxed);
        assert!(matches!(
            backend.push("test", b"three", None),
            Err(BackendError::S3(_))
        ));
        assert_eq!(
            store.objects.lock().unwrap()["/dots/test/manifest.json"],
            manifest
        );
        store.fail.store(3, Ordering::Relaxed);
        assert!(matches!(backend.prune("test", 1), Err(BackendError::S3(_))));
        assert_eq!(
            store.objects.lock().unwrap()["/dots/test/manifest.json"],
            manifest
        );
    }
    #[test]
    fn delta_metadata_retention_and_stale_pushes() {
        use super::super::SnapshotType;
        let (store, backend) = Store::start(None);
        backend.push("test", b"base", None).unwrap();
        let options = SnapshotOptions {
            snapshot_type: SnapshotType::Delta,
            base_version: Some(1),
            deleted_files: vec![".oldrc".into()],
        };
        assert_eq!(
            backend
                .push_snapshot("test", b"delta", None, options.clone(), Some(1))
                .unwrap(),
            2
        );
        let versions = backend.list_versions("test").unwrap();
        assert_eq!(versions[0].snapshot_type, SnapshotType::Full);
        assert_eq!(versions[1].snapshot_type, SnapshotType::Delta);
        assert_eq!(versions[1].base_version, Some(1));
        assert_eq!(versions[1].deleted_files, vec![".oldrc"]);
        let objects = store.objects.lock().unwrap().clone();
        assert!(matches!(
            backend.push_snapshot("test", b"stale", None, options, Some(1)),
            Err(BackendError::VersionConflict {
                local: 1,
                remote: 2
            })
        ));
        assert_eq!(*store.objects.lock().unwrap(), objects);
        backend.prune("test", 1).unwrap();
        assert_eq!(backend.list_versions("test").unwrap().len(), 2);
        assert_eq!(backend.pull("test", Some(1)).unwrap().1, b"base");
        backend.push("test", b"checkpoint", None).unwrap();
        backend.prune("test", 1).unwrap();
        assert_eq!(backend.list_versions("test").unwrap().len(), 1);
        assert!(!store
            .objects
            .lock()
            .unwrap()
            .contains_key("/dots/test/v001.tar.zst"));
        assert!(!store
            .objects
            .lock()
            .unwrap()
            .contains_key("/dots/test/v002.tar.zst"));
    }
}
