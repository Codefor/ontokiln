//! Request-owned Full artifacts. Only the supervisor publishes after reaping.
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const MANIFEST_VERSION: &str = "kernel-manifest-v1";
pub const FORMAT_VERSION: &str = "ontology-kernel-sqlite/v1";
pub const ENGINE_VERSION: &str = "positive-rdf-v5";

pub fn canonical(value: &Value) -> Vec<u8> {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .map(|(k, v)| (k.clone(), sorted(v)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            _ => value.clone(),
        }
    }
    serde_json::to_vec(&sorted(value)).expect("JSON value")
}

pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn file_digest(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let size = input.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn regular(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "SOURCE_INVALID|expected regular non-symlink file: {}",
        path.display()
    );
    Ok(())
}

pub(crate) fn freeze(source: &Path, destination: &Path, expected: &str) -> Result<String> {
    regular(source)?;
    let before = fs::metadata(source)?;
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let size = input.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        output.write_all(&buffer[..size])?;
        hash.update(&buffer[..size]);
    }
    let actual = hex::encode(hash.finalize());
    let after = fs::metadata(source)?;
    ensure!(
        before.len() == after.len()
            && before.modified()? == after.modified()?
            && actual == expected,
        "SOURCE_INVALID|frozen source changed or digest mismatched: {}",
        source.display()
    );
    Ok(actual)
}

fn inventory(directory: &Path) -> Result<Value> {
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(directory)
        .min_depth(1)
        .follow_links(false)
    {
        let entry = entry?;
        ensure!(
            !entry.file_type().is_symlink(),
            "MANIFEST_CORRUPT|artifact symlink"
        );
        if entry.file_type().is_file() {
            let name = entry
                .path()
                .strip_prefix(directory)?
                .to_str()
                .context("UTF-8 inventory path")?
                .to_owned();
            if name != "manifest.json" {
                files.insert(
                    name,
                    json!({"bytes": entry.metadata()?.len(), "sha256": file_digest(entry.path())?}),
                );
            }
        } else {
            ensure!(
                entry.file_type().is_dir(),
                "MANIFEST_CORRUPT|artifact special file"
            );
        }
    }
    Ok(json!(files))
}

pub(crate) fn verify(directory: &Path) -> Result<Value> {
    regular(&directory.join("manifest.json"))?;
    let manifest: Value = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    ensure!(
        matches!(
            manifest["schema_version"].as_str(),
            Some("kernel-manifest-v1" | "kernel-manifest-v2")
        ) && manifest["format_version"] == FORMAT_VERSION
            && manifest["engine_semantic_version"] == ENGINE_VERSION,
        "FORMAT_VERSION_UNSUPPORTED|unsupported Full artifact"
    );
    let files = inventory(directory)?;
    if manifest["schema_version"] == "kernel-manifest-v2" {
        ensure!(
            manifest["kernel"] == crate::identity::kernel()
                && manifest["proof_coverage"]
                    == json!({"mode":"closure-only", "per_fact_proof":"not_recorded"})
                && (manifest["kind"] != "reasoned" || manifest["fixpoint_reached"] == true),
            "FORMAT_VERSION_UNSUPPORTED|native artifact identity or coverage mismatch"
        );
    }
    ensure!(
        manifest["completion"] == "COMPLETE"
            && files == manifest["files"]
            && digest(&canonical(&files)) == manifest["content_digest"],
        "MANIFEST_CORRUPT|artifact inventory or completion mismatch"
    );
    for name in ["facts.sqlite", "input_manifest.json", "plan.json"] {
        ensure!(
            files.get(name).is_some(),
            "MANIFEST_CORRUPT|missing required artifact file"
        );
    }
    Ok(manifest)
}

fn integrity(stage: &Path) -> Result<()> {
    let c =
        Connection::open_with_flags(stage.join("facts.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let check: String = c.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure!(
        check == "ok",
        "INDEX_BUILD_FAILED|native index consistency failed"
    );
    Ok(())
}

/// The output lock and staging directory belong to one invocation, never a caller.
pub struct Stage {
    directory: tempfile::TempDir,
    output: PathBuf,
    lock: PathBuf,
    published: bool,
    committed: bool,
    receipt: Option<PathBuf>,
}

impl Stage {
    pub fn reserve(output: &Path) -> Result<Self> {
        let output = std::path::absolute(output)?;
        let parent = output.parent().context("output parent")?;
        fs::create_dir_all(parent)?;
        let name = output
            .file_name()
            .context("output filename")?
            .to_str()
            .context("UTF-8 output")?;
        let lock = parent.join(format!(".{name}.lock"));
        let mut reservation = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|e| anyhow::anyhow!("BUILD_ABORTED|output reservation failed: {e}"))?;
        let result = (|| {
            reservation.write_all(std::process::id().to_string().as_bytes())?;
            ensure!(
                !output.exists() && fs::symlink_metadata(&output).is_err(),
                "BUILD_ABORTED|immutable output already exists"
            );
            let directory = tempfile::Builder::new()
                .prefix(&format!(".{name}.staging-"))
                .tempdir_in(parent)?;
            Ok(Self {
                directory,
                output: output.clone(),
                lock: lock.clone(),
                published: false,
                committed: false,
                receipt: None,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(lock);
        }
        result
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn own(&mut self, request_digest: &str) -> Result<()> {
        let path = self.output.with_file_name(format!(
            "{}.native-execution.json",
            self.output
                .file_name()
                .context("output name")?
                .to_string_lossy()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        self.receipt = Some(path);
        file.write_all(&canonical(
            &json!({"request_sha256":request_digest, "stage":self.path(), "status":"RUNNING", "supervisor_pid":std::process::id()}),
        ))?;
        file.sync_all()?;
        Ok(())
    }

    pub fn finish_record(&self, record: &Value) -> Result<()> {
        let mut file =
            tempfile::NamedTempFile::new_in(self.output.parent().context("output parent")?)?;
        file.write_all(&canonical(record))?;
        file.as_file().sync_all()?;
        file.persist(self.receipt.as_ref().context("owned native receipt")?)
            .map_err(|e| e.error)?;
        Ok(())
    }

    pub fn publish(&mut self) -> Result<Value> {
        let manifest = verify(self.path())?;
        for entry in walkdir::WalkDir::new(self.path()).contents_first(true) {
            File::open(entry?.path())?.sync_all()?;
        }
        rename_no_replace(self.path(), &self.output)?;
        self.published = true;
        File::open(self.output.parent().context("output parent")?)?.sync_all()?;
        Ok(
            json!({"path": self.output, "manifest_sha256": file_digest(&self.output.join("manifest.json"))?,
            "content_digest": manifest["content_digest"], "semantic_fingerprint": manifest["semantic_fingerprint"],
            "source_tree_digest": manifest["source_tree_digest"], "profile_id": manifest["profile_id"]}),
        )
    }

    pub fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        if self.published && !self.committed {
            let _ = fs::remove_dir_all(&self.output);
        }
        if !self.committed
            && let Some(path) = &self.receipt
        {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_file(&self.lock);
    }
}

fn rename_no_replace(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    let status = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let status = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    bail!("FORMAT_VERSION_UNSUPPORTED|atomic no-replace rename is unavailable");
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ensure!(
        status == 0,
        "BUILD_ABORTED|atomic artifact publication failed: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

pub fn compute(request: Value) -> Result<Value> {
    compute_with(request, crate::full::execute)
}

/// Embed the artifact lifecycle with a caller-owned execution adapter.
/// Inputs are frozen before the adapter runs; integrity and publication remain here.
pub fn compute_with(
    mut request: Value,
    execute: impl Fn(Value) -> std::result::Result<Value, String>,
) -> Result<Value> {
    let artifact = request["artifact"].clone();
    let stage = PathBuf::from(artifact["stage"].as_str().context("artifact stage")?);
    let operation = request["operation"]
        .as_str()
        .context("operation")?
        .to_owned();
    request["database"] = json!(stage.join("facts.sqlite"));
    let (mut manifest, result) = if operation == "compile" {
        fs::create_dir(stage.join("inputs"))?;
        let profile = artifact["profile_id"].as_str().context("profile_id")?;
        ensure!(
            artifact["index_profile"] == "core-v1",
            "FORMAT_VERSION_UNSUPPORTED|unsupported index profile"
        );
        let mut sources = request["sources"].as_array().context("sources")?.clone();
        let rule_paths = artifact["rules_files"]
            .as_array()
            .context("rules_files")?
            .iter()
            .filter_map(|rule| rule["path"].as_str())
            .map(Path::new)
            .filter(|path| path.extension().is_some_and(|extension| extension == "ttl"))
            .map(fs::canonicalize)
            .collect::<std::io::Result<std::collections::BTreeSet<_>>>()?;
        sources.sort_by(|a, b| a["document_id"].as_str().cmp(&b["document_id"].as_str()));
        ensure!(
            !sources.is_empty() && sources.len() <= 100_000,
            "SOURCE_INVALID|source count"
        );
        let mut documents = std::collections::BTreeSet::new();
        let mut source_manifest = Vec::new();
        let mut source_hashes = BTreeMap::new();
        for (i, source) in sources.iter_mut().enumerate() {
            let document = source["document_id"]
                .as_str()
                .context("document_id")?
                .to_owned();
            ensure!(
                !document.is_empty() && documents.insert(document.to_owned()),
                "SOURCE_INVALID|duplicate or empty document_id"
            );
            let file = format!("inputs/source-{i:06}.rdf");
            let source_path = fs::canonicalize(source["path"].as_str().context("source path")?)?;
            regular(Path::new(source["path"].as_str().context("source path")?))?;
            let digest = freeze(
                &source_path,
                &stage.join(&file),
                source["expected_sha256"]
                    .as_str()
                    .context("expected source digest")?,
            )?;
            source_hashes.insert(source_path.clone(), digest.clone());
            source["is_rule"] = json!(rule_paths.contains(&source_path));
            source_manifest.push(json!({"document_id": document, "base_iri": source["base_iri"], "format": source["format"], "sha256": digest, "file": file}));
            source["path"] = json!(stage.join(file));
        }
        let tree_digest = digest(&canonical(&json!(source_manifest)));
        if let Some(expected) = artifact["expected_source_tree_digest"].as_str() {
            ensure!(
                expected == tree_digest,
                "SOURCE_INVALID|source tree digest mismatch"
            );
        }
        let mut rule_manifest = Vec::new();
        let mut rule_paths = Vec::new();
        for (i, rule) in artifact["rules_files"]
            .as_array()
            .context("rules_files")?
            .iter()
            .enumerate()
        {
            let source = Path::new(rule["path"].as_str().context("rule path")?);
            let suffix = match source.extension().and_then(|s| s.to_str()) {
                Some("ttl") => "ttl",
                Some("rq") => "rq",
                _ => "json",
            };
            let file = format!("inputs/rules-{i:06}.{suffix}");
            let hash = freeze(
                source,
                &stage.join(&file),
                rule["expected_sha256"]
                    .as_str()
                    .context("expected rule digest")?,
            )?;
            if let Some(source_hash) = source_hashes.get(&fs::canonicalize(source)?) {
                ensure!(
                    source_hash == &hash,
                    "SOURCE_INVALID|rule and source bytes differ"
                );
            }
            rule_manifest.push(json!({"sha256": hash, "file": file}));
            rule_paths.push(stage.join(&file));
        }
        let admitted = crate::full::execute(
            json!({"operation":"rules", "profile":profile, "files":rule_paths}),
        )
        .map_err(anyhow::Error::msg)?;
        request["sources"] = json!(sources);
        request["rules"] = admitted["rules"].clone();
        request["extract_property_chains"] = json!(profile == "rdfs-property-chain-v1");
        let result = execute(request.clone()).map_err(anyhow::Error::msg)?;
        let frozen = json!({"sources":source_manifest, "rules":rule_manifest, "import_lock":result["resolved_import_lock"],
            "import_policy":if request["import_lock"].is_null() {"declared-ontology-v1"} else {"explicit-lock-v1"}, "profile_id":profile, "parser_options":result["parser_options"]});
        fs::write(stage.join("input_manifest.json"), canonical(&frozen))?;
        fs::write(stage.join("plan.json"), canonical(&result["plan"]))?;
        (
            json!({"kind":"compiled", "stats":result["stats"], "source_tree_digest":tree_digest,
            "semantic_fingerprint":digest(&canonical(&json!({"inputs":frozen, "engine":ENGINE_VERSION, "index_profile":"core-v1"}))),
            "profile_id":profile, "source_provenance_version":"explicit-doc-v1"}),
            result,
        )
    } else if operation == "materialize" {
        let compiled = Path::new(artifact["compiled"].as_str().context("compiled artifact")?);
        let source_manifest = verify(compiled)?;
        ensure!(
            source_manifest["kind"] == "compiled",
            "SOURCE_INVALID|expected explicit compiled artifact"
        );
        for (name, info) in source_manifest["files"]
            .as_object()
            .context("compiled inventory")?
        {
            let destination = stage.join(name);
            fs::create_dir_all(destination.parent().context("inventory parent")?)?;
            freeze(
                &compiled.join(name),
                &destination,
                info["sha256"].as_str().context("inventory digest")?,
            )?;
        }
        request["plan"] = serde_json::from_slice(&fs::read(stage.join("plan.json"))?)?;
        let result = execute(request).map_err(anyhow::Error::msg)?;
        ensure!(
            verify(compiled)? == source_manifest,
            "MANIFEST_CORRUPT|input changed during reasoning"
        );
        (
            json!({"kind":"reasoned", "stats":result["stats"], "profile_id":source_manifest["profile_id"],
            "source_tree_digest":source_manifest["source_tree_digest"], "semantic_fingerprint":source_manifest["semantic_fingerprint"],
            "compiled_content_digest":source_manifest["content_digest"], "fixpoint_reached":result["fixpoint_reached"],
            "source_provenance_version":source_manifest["source_provenance_version"]}),
            result,
        )
    } else {
        bail!("SOURCE_INVALID|artifact operation must be compile or materialize");
    };
    integrity(&stage)?;
    let files = inventory(&stage)?;
    manifest["schema_version"] = json!(MANIFEST_VERSION);
    manifest["format_version"] = json!(FORMAT_VERSION);
    manifest["engine_semantic_version"] = json!(ENGINE_VERSION);
    manifest["completion"] = json!("COMPLETE");
    manifest["files"] = files.clone();
    manifest["content_digest"] = json!(digest(&canonical(&files)));
    fs::write(stage.join("manifest.json"), canonical(&manifest))?;
    let mut result = result;
    result["integrity_checked"] = json!(true);
    result["proof_coverage"] = json!({"mode":"closure-only", "per_fact_proof":"not_recorded"});
    Ok(result)
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod publication_tests {
    use super::*;

    #[test]
    fn atomic_publication_preserves_an_existing_destination() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::create_dir(&source)?;
        fs::write(source.join("fact"), b"candidate")?;
        fs::create_dir(&destination)?;
        assert!(rename_no_replace(&source, &destination).is_err());
        assert_eq!(fs::read(source.join("fact"))?, b"candidate");
        assert_eq!(fs::read_dir(&destination)?.count(), 0);

        fs::remove_dir(&destination)?;
        rename_no_replace(&source, &destination)?;
        assert!(!source.exists());
        assert_eq!(fs::read(destination.join("fact"))?, b"candidate");
        Ok(())
    }
}
