//! Read dependencies belong to the loader, not to a second list of BBS paths.
//! Nothing is watched or written here. Caches are bounded and can be discarded.
use std::{
    collections::{BTreeMap, VecDeque},
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
    kind: u32,
}
fn stamp(meta: fs::Metadata) -> Stamp {
    Stamp {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        mtime: (meta.mtime(), meta.mtime_nsec()),
        ctime: (meta.ctime(), meta.ctime_nsec()),
        kind: meta.mode() & u32::from(libc::S_IFMT),
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Observed {
    link: Option<Stamp>,
    target: Option<Stamp>,
}
fn optional(result: io::Result<fs::Metadata>) -> io::Result<Option<Stamp>> {
    match result {
        Ok(meta) => Ok(Some(stamp(meta))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn observe(path: &Path) -> io::Result<Observed> {
    let meta = fs::symlink_metadata(path);
    let symlink = meta.as_ref().is_ok_and(|m| m.file_type().is_symlink());
    let link = optional(meta)?;
    let target = if symlink {
        optional(fs::metadata(path))?
    } else {
        link.clone()
    };
    Ok(Observed { link, target })
}

#[derive(Clone)]
pub(crate) struct Deps {
    enabled: bool,
    consistent: bool,
    paths: BTreeMap<PathBuf, Observed>,
    read_bytes: usize,
}
impl Deps {
    pub(crate) fn new() -> Self {
        Self {
            enabled: true,
            consistent: true,
            paths: BTreeMap::new(),
            read_bytes: 0,
        }
    }
    pub(crate) fn untracked() -> Self {
        Self {
            enabled: false,
            ..Self::new()
        }
    }
    /// Must precede the read, metadata decision, or directory enumeration.
    pub(crate) fn stat(&mut self, path: &Path) {
        if !self.enabled {
            return;
        }
        match observe(path) {
            Ok(now) => {
                if let Some(before) = self.paths.get(path) {
                    self.consistent &= before == &now;
                } else {
                    self.paths.insert(path.to_owned(), now);
                }
            }
            Err(_) => self.consistent = false,
        }
    }
    pub(crate) fn inherit(&mut self, other: &Self) {
        if !self.enabled {
            return;
        }
        self.consistent &= other.consistent;
        for (path, observed) in &other.paths {
            if let Some(before) = self.paths.get(path) {
                self.consistent &= before == observed;
            } else {
                self.paths.insert(path.clone(), observed.clone());
            }
        }
    }
    pub(crate) fn inherit_value(&mut self, other: &Self) {
        self.inherit(other);
        self.read_bytes = self.read_bytes.saturating_add(other.read_bytes);
    }
    pub(crate) fn unchanged(&self) -> bool {
        self.unchanged_with(|| Ok::<_, std::convert::Infallible>(()))
            .unwrap_or(false)
    }
    pub(crate) fn unchanged_with<E>(
        &self,
        mut check: impl FnMut() -> Result<(), E>,
    ) -> Result<bool, E> {
        if !self.enabled || !self.consistent {
            return Ok(false);
        }
        for (path, before) in &self.paths {
            check()?;
            if !observe(path).is_ok_and(|now| &now == before) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn includes(&self, input: &Self) -> bool {
        input.consistent
            && input
                .paths
                .iter()
                .all(|(path, value)| self.paths.get(path) == Some(value))
    }
    pub(crate) fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        self.stat(path);
        let bytes = fs::read(path)?;
        self.read_bytes = self.read_bytes.saturating_add(bytes.len());
        Ok(bytes)
    }
    pub(crate) fn read_to_string(&mut self, path: &Path) -> io::Result<String> {
        self.stat(path);
        let text = fs::read_to_string(path)?;
        self.read_bytes = self.read_bytes.saturating_add(text.len());
        Ok(text)
    }
    pub(crate) fn read_dir(&mut self, path: &Path) -> io::Result<fs::ReadDir> {
        self.stat(path);
        fs::read_dir(path)
    }
    pub(crate) fn is_dir(&mut self, path: &Path) -> bool {
        self.stat(path);
        path.is_dir()
    }
    pub(crate) fn is_file(&mut self, path: &Path) -> bool {
        self.stat(path);
        path.is_file()
    }
    fn weight(&self) -> usize {
        // Bound retained input plus bookkeeping; no attachment bytes are read.
        self.read_bytes
            .saturating_mul(2)
            .saturating_add(self.paths.len().saturating_mul(256))
            .saturating_add(1024)
    }
}

pub(super) struct Cached<V> {
    pub value: Arc<V>,
    pub deps: Deps,
}
impl<V> Clone for Cached<V> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            deps: self.deps.clone(),
        }
    }
}
pub(super) struct Memo<K, V> {
    entries: Mutex<VecDeque<(K, Cached<V>)>>,
    limit: usize,
    bytes: usize,
}
impl<K: PartialEq, V> Memo<K, V> {
    pub const fn new(limit: usize, bytes: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            limit,
            bytes,
        }
    }
    #[cfg(test)]
    pub(super) fn peek(&self, key: &K) -> Option<Cached<V>> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }
    pub fn get(
        &self,
        key: K,
        load: impl FnOnce(&mut Deps) -> anyhow::Result<V>,
    ) -> anyhow::Result<Cached<V>> {
        self.get_seeded(key, &Deps::new(), load)
    }
    pub fn get_seeded(
        &self,
        key: K,
        input: &Deps,
        load: impl FnOnce(&mut Deps) -> anyhow::Result<V>,
    ) -> anyhow::Result<Cached<V>> {
        let old = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, v)| v.clone());
        if let Some(old) = old.filter(|v| v.deps.includes(input) && v.deps.unchanged()) {
            return Ok(old);
        }
        let mut deps = Deps::new();
        deps.inherit(input);
        let value = Arc::new(load(&mut deps)?);
        let cached = Cached { value, deps };
        let reusable = cached.deps.weight() <= self.bytes && cached.deps.unchanged();
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|(k, _)| k != &key);
        if reusable {
            while entries.len() >= self.limit
                || entries
                    .iter()
                    .map(|(_, v)| v.deps.weight())
                    .sum::<usize>()
                    .saturating_add(cached.deps.weight())
                    > self.bytes
            {
                entries.pop_front();
            }
            entries.push_back((key, cached.clone()));
        }
        Ok(cached)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("bbs-deps-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unchanged_inputs_reuse_value_but_atomic_replace_and_missing_creation_reload() {
        let scratch = Scratch::new();
        let path = scratch.0.join("input");
        let memo = Memo::new(4, 16 * 1024);
        let loads = Cell::new(0);
        let read = || {
            memo.get((), |deps| {
                loads.set(loads.get() + 1);
                Ok(deps.read_to_string(&path).ok())
            })
            .unwrap()
        };
        let first = read();
        assert!(Arc::ptr_eq(&first.value, &read().value));
        fs::write(&path, "one").unwrap();
        let second = read();
        assert_eq!(second.value.as_ref().as_deref(), Some("one"));
        let replacement = scratch.0.join("replacement");
        fs::write(&replacement, "two").unwrap();
        fs::rename(replacement, &path).unwrap();
        let third = read();
        assert_eq!(third.value.as_ref().as_deref(), Some("two"));
        assert_eq!(loads.get(), 3);
        fs::remove_file(&path).unwrap();
        assert!(read().value.is_none());
    }

    #[test]
    fn read_time_changes_and_stale_parent_inputs_are_never_cached_as_current() {
        let scratch = Scratch::new();
        let path = scratch.0.join("input");
        fs::write(&path, "before").unwrap();
        let memo = Memo::new(2, 16 * 1024);
        let first = memo
            .get((), |deps| {
                let value = deps.read_to_string(&path)?;
                fs::write(&path, "after").unwrap();
                Ok(value)
            })
            .unwrap();
        assert_eq!(&*first.value, "before");
        assert!(memo.peek(&()).is_none());
        let now = memo
            .get((), |deps| Ok(deps.read_to_string(&path)?))
            .unwrap();
        assert_eq!(&*now.value, "after");
        // A parent may have read an older version before this child was built.
        // Never reuse the child for that different input or publish it as new.
        let stale = memo
            .get_seeded((), &first.deps, |_| Ok("parent before".into()))
            .unwrap();
        assert_eq!(&*stale.value, "parent before");
        assert!(memo.peek(&()).is_none());
    }

    #[test]
    fn child_dependencies_survive_cache_hits_and_nested_directory_writes() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("posts");
        fs::create_dir(&dir).unwrap();
        let child = Memo::new(2, 16 * 1024);
        let load = |deps: &mut Deps| -> anyhow::Result<usize> { Ok(deps.read_dir(&dir)?.count()) };
        child.get((), load).unwrap();
        let hit = child
            .get((), |_| panic!("should reuse directory listing"))
            .unwrap();
        let mut parent = Deps::new();
        parent.inherit(&hit.deps);
        assert!(parent.unchanged());
        let outer = fs::metadata(&scratch.0).unwrap().modified().unwrap();
        fs::write(dir.join("external.md"), "new CLI post").unwrap();
        assert_eq!(fs::metadata(&scratch.0).unwrap().modified().unwrap(), outer);
        assert!(!parent.unchanged());
        assert_eq!(*child.get((), load).unwrap().value, 1);
    }

    #[test]
    fn cache_is_bounded_and_failed_or_oversized_loads_are_not_retained() {
        let memo = Memo::new(2, 4096);
        for key in 0..3 {
            memo.get(key, |_| Ok(key)).unwrap();
        }
        assert!(memo.peek(&0).is_none());
        assert!(memo.get(3, |_| anyhow::bail!("broken")).is_err());
        assert!(memo.peek(&3).is_none());
        let scratch = Scratch::new();
        let path = scratch.0.join("large");
        fs::write(&path, vec![b'x'; 4096]).unwrap();
        let bytes = Memo::new(2, 4096);
        assert_eq!(
            bytes
                .get((), |deps| Ok(deps.read(&path)?))
                .unwrap()
                .value
                .len(),
            4096
        );
        assert!(bytes.peek(&()).is_none());
    }
}
