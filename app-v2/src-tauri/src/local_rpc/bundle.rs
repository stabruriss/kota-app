use super::error::{LocalError, LocalResult};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::{AppHandle, Manager};

pub(crate) const PI_VERSION: &str = "1.3.0";
const PIN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../vendor/pi-runtime/pi-runtime.lock.json"
));
#[derive(Deserialize)]
struct Pin {
    version: String,
    vendor_manifest_sha256: String,
}
fn pin() -> &'static Pin {
    static VALUE: OnceLock<Pin> = OnceLock::new();
    VALUE.get_or_init(|| serde_json::from_str(PIN).expect("checked-in Pi runtime pin"))
}

#[derive(Clone)]
pub(crate) struct BundledPi {
    pub executable: PathBuf,
    pub extension: PathBuf,
}

impl BundledPi {
    /// The archive/prepare gate hashes the full tree. At runtime validate the
    /// small pinned manifest and necessary entrypoints, permitting signed
    /// Mach-O bytes. Never discover or execute a user's PATH installation.
    pub fn from_root(root: &Path, extension: &Path) -> LocalResult<Self> {
        let invalid = || LocalError::new("pi_runtime_unavailable");
        if !root.is_absolute() || !extension.is_absolute() {
            return Err(invalid());
        }
        let manifest = root.join("VENDOR-MANIFEST.json");
        let metadata = fs::symlink_metadata(&manifest).map_err(|_| invalid())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1024 * 1024
        {
            return Err(invalid());
        }
        let bytes = fs::read(manifest).map_err(|_| invalid())?;
        if format!("{:x}", Sha256::digest(&bytes)) != pin().vendor_manifest_sha256
            || pin().version != PI_VERSION
        {
            return Err(invalid());
        }
        for file in [
            "pi",
            "photon_rs_bg.wasm",
            "native/darwin/prebuilds/darwin-arm64/darwin-platform.node",
            "package.json",
        ] {
            let metadata = fs::symlink_metadata(root.join(file)).map_err(|_| invalid())?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(invalid());
            }
        }
        let executable = root.join("pi");
        #[cfg(unix)]
        if fs::metadata(&executable)
            .map_err(|_| invalid())?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err(invalid());
        }
        if !extension.is_file() {
            return Err(LocalError::new("local_extension_unavailable"));
        }
        Ok(Self {
            executable,
            extension: extension.to_owned(),
        })
    }
    pub fn for_app(app: &AppHandle) -> LocalResult<Self> {
        let resource = app
            .path()
            .resource_dir()
            .map_err(|_| LocalError::new("pi_runtime_unavailable"))?;
        let root = resource.join("kota-pi");
        let extension = resource.join("kota-ext/index.ts");
        if root.exists() || packaged_executable(&std::env::current_exe().unwrap_or_default()) {
            return Self::from_root(&root, &extension);
        }
        #[cfg(debug_assertions)]
        {
            // Explicit development injection only; no guessed staging, deps
            // directory, PATH executable, or override inside a packaged App.
            let root = std::env::var_os("KOTA_LOCAL_PI_RUNTIME_ROOT")
                .map(PathBuf::from)
                .ok_or_else(|| LocalError::new("pi_runtime_unavailable"))?;
            let extension = Path::new(env!("CARGO_MANIFEST_DIR")).join("../kota-ext/index.ts");
            return Self::from_root(&root, &extension);
        }
        #[cfg(not(debug_assertions))]
        Err(LocalError::new("pi_runtime_unavailable"))
    }
}
fn packaged_executable(executable: &Path) -> bool {
    executable.parent().is_some_and(|p| {
        p.file_name().is_some_and(|n| n == "MacOS")
            && p.parent()
                .is_some_and(|p| p.file_name().is_some_and(|n| n == "Contents"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn pinned_resource_entrypoints_are_required_without_a_system_pi_fallback() {
        let root =
            std::env::temp_dir().join(format!("local-bundle-fixture-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let f = Fixture(root);
        let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../vendor/pi-runtime/kota-pi-1.3.0-darwin-arm64-r1.tar.gz");
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(archive).unwrap(),
        ));
        let mut manifest = None;
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap() == Path::new("kota-pi/VENDOR-MANIFEST.json") {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                manifest = Some(bytes);
                break;
            }
        }
        fs::write(f.0.join("VENDOR-MANIFEST.json"), manifest.unwrap()).unwrap();
        for name in [
            "pi",
            "photon_rs_bg.wasm",
            "native/darwin/prebuilds/darwin-arm64/darwin-platform.node",
            "package.json",
            "index.ts",
        ] {
            let path = f.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "not executed").unwrap();
        }
        let extension = f.0.join("index.ts");
        #[cfg(unix)]
        {
            assert!(BundledPi::from_root(&f.0, &extension).is_err());
            fs::set_permissions(f.0.join("pi"), fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(BundledPi::from_root(&f.0, &extension).is_ok());
        // Re-signing code is allowed; missing adjacent data is not.
        fs::write(f.0.join("pi"), "changed code bytes, not executed").unwrap();
        assert!(BundledPi::from_root(&f.0, &extension).is_ok());
        fs::remove_file(f.0.join("photon_rs_bg.wasm")).unwrap();
        assert_eq!(
            BundledPi::from_root(&f.0, &extension).err().unwrap().code,
            "pi_runtime_unavailable"
        );
        fs::write(f.0.join("VENDOR-MANIFEST.json"), "{}").unwrap();
        assert!(BundledPi::from_root(&f.0, &extension).is_err());
        assert!(BundledPi::from_root(Path::new("relative"), &extension).is_err());
    }
    #[test]
    fn a_packaged_debug_app_cannot_take_the_development_override() {
        assert!(packaged_executable(Path::new(
            "/tmp/Kota.app/Contents/MacOS/kota-v2"
        )));
        assert!(!packaged_executable(Path::new("/tmp/target/debug/kota-v2")));
    }
}
