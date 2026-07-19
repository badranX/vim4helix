//! Native (pure-Rust) file watcher: reloads open buffers when their backing
//! file changes on disk.
//!
//! This replaces the Steel `helix-file-watcher` plugin so that the native `hx`
//! binary gets auto-reload without loading any Scheme.
//!
//! Design constraint: keep upstream-merge conflicts to a minimum. All logic
//! lives in this new file; the only upstream wiring is a single `pub mod`
//! declaration in `lib.rs`. Registration is triggered from `events.rs`
//! (already fork-owned) via `register()`.
//!
//! Flow: a background `notify` watcher thread forwards changed paths into an
//! `AsyncHook` (debounced, runs on the tokio runtime). When the debounce
//! fires, the hook enqueues an editor job (`job::dispatch_blocking`) that runs
//! on the main thread with `&mut Editor` and reloads the affected, unmodified
//! documents. Watched paths are kept in sync with open buffers through the
//! `DocumentDidOpen` / `DocumentDidClose` hooks.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use helix_event::{register_hook, send_blocking, AsyncHook};
use helix_view::{
    events::{DocumentDidClose, DocumentDidOpen},
    Editor,
};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::{sync::mpsc::Sender, time::Instant};

use crate::job;

/// Debounce window: collect FS events for this long after the last one before
/// reloading. Also absorbs the editor's own save writes so we do not race a
/// `:w`.
const DEBOUNCE: Duration = Duration::from_millis(500);

/// Message sent from the `notify` watcher thread to the async hook.
enum WatchEvent {
    Changed(Vec<PathBuf>),
}

/// Shared handle used by the hooks to (un)watch files as buffers open/close.
///
/// The `notify` watcher is not `Sync`-friendly to share across the hook
/// closures, so we keep it behind a mutex and expose watch/unwatch helpers.
#[derive(Clone)]
struct WatcherHandle {
    inner: Arc<Mutex<WatcherState>>,
}

struct WatcherState {
    watcher: RecommendedWatcher,
    /// Canonicalized file paths of the currently open buffers we care about.
    /// The notify callback filters incoming events against this set so we only
    /// react to files that are actually open.
    files: HashSet<PathBuf>,
    /// Parent directory -> number of watched files inside it. We watch the
    /// *directory*, not the file, because atomic saves (helix's own `:w` and
    /// many external editors) replace the file via rename+create: watching the
    /// file inode directly would go stale after the first save. Watching the
    /// dir survives that. The refcount lets us drop the dir watch only once its
    /// last file closes.
    dirs: HashMap<PathBuf, usize>,
}

impl WatcherHandle {
    fn watch(&self, path: &Path) {
        let Ok(canonical) = path.canonicalize() else {
            return;
        };
        let Some(dir) = canonical.parent().map(Path::to_path_buf) else {
            return;
        };
        let mut state = self.inner.lock().unwrap();
        if !state.files.insert(canonical) {
            return;
        }
        // First file in this directory: start watching the directory.
        match state.dirs.get(&dir).copied() {
            Some(count) => {
                state.dirs.insert(dir, count + 1);
            }
            None => {
                if state.watcher.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
                    state.dirs.insert(dir, 1);
                }
                // On failure we leave `dirs` without the entry so a later file
                // in the same directory retries the watch.
            }
        }
    }

    fn unwatch(&self, path: &Path) {
        let Ok(canonical) = path.canonicalize() else {
            return;
        };
        let Some(dir) = canonical.parent().map(Path::to_path_buf) else {
            return;
        };
        let mut state = self.inner.lock().unwrap();
        if !state.files.remove(&canonical) {
            return;
        }
        if let Some(count) = state.dirs.get_mut(&dir) {
            *count -= 1;
            if *count == 0 {
                let _ = state.watcher.unwatch(&dir);
                state.dirs.remove(&dir);
            }
        }
    }

    /// Keep only the paths that belong to files we currently watch.
    fn filter_open(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let state = self.inner.lock().unwrap();
        paths
            .into_iter()
            .filter(|p| match p.canonicalize() {
                Ok(c) => state.files.contains(&c),
                // A removed/renamed file can no longer be canonicalized; fall
                // back to the raw path so deletions still trigger a reload.
                Err(_) => state.files.contains(p),
            })
            .collect()
    }
}

/// The debounced async hook that owns reload scheduling.
///
/// Holds a (late-bound) handle so it can filter directory-level FS events down
/// to the files that are actually open before scheduling a reload.
struct FileWatcherHandler {
    pending: HashSet<PathBuf>,
    handle: Arc<Mutex<Option<WatcherHandle>>>,
}

impl AsyncHook for FileWatcherHandler {
    type Event = WatchEvent;

    fn handle_event(&mut self, event: Self::Event, _timeout: Option<Instant>) -> Option<Instant> {
        let WatchEvent::Changed(paths) = event;
        self.pending.extend(paths);
        Some(Instant::now() + DEBOUNCE)
    }

    fn finish_debounce(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = self.pending.drain().collect();
        // We watch whole directories, so drop events for files we don't have
        // open before bothering the main thread.
        let paths = match self.handle.lock().unwrap().as_ref() {
            Some(handle) => handle.filter_open(paths),
            None => return,
        };
        if paths.is_empty() {
            return;
        }
        // Hop back to the main thread to touch the editor.
        job::dispatch_blocking(move |editor, _compositor| {
            reload_paths(editor, &paths);
        });
    }
}

/// Reload every open document whose path is in `paths` from disk.
///
/// The reload is unconditional (even for buffers with unsaved local edits):
/// `doc.reload` records the swap in the undo history, so the user can `u` to
/// recover in-session edits. This is simpler and less surprising than skipping
/// dirty buffers, which could leave the buffer silently out of sync with disk.
fn reload_paths(editor: &mut Editor, paths: &[PathBuf]) {
    let scrolloff = editor.config().scrolloff;

    let targets: Vec<helix_view::DocumentId> = paths
        .iter()
        .filter_map(|path| editor.document_by_path(path))
        .map(|doc| doc.id())
        .collect();

    // Focused view, read before taking any mutable document borrow.
    let focus = editor.tree.focus;

    for doc_id in targets {
        // Reload needs a view; use any view the doc is shown in, else the
        // focused one after ensuring it is initialized.
        let view_id = {
            let doc = doc_mut!(editor, &doc_id);
            let view_id = doc.selections().keys().copied().next().unwrap_or(focus);
            doc.ensure_view_init(view_id);
            view_id
        };

        // `doc_mut!`/`view_mut!` borrow disjoint editor fields (`documents`
        // vs `tree`), so both borrows can be live at once, as in `:reload-all`.
        let doc = doc_mut!(editor, &doc_id);
        let view = view_mut!(editor, view_id);
        view.sync_changes(doc);

        if let Err(err) = doc.reload(view, &editor.diff_providers) {
            log::error!("file-watcher: failed to reload {:?}: {err}", doc.path());
            continue;
        }
        view.ensure_cursor_in_view(doc, scrolloff);
    }
}

/// Build the notify watcher and spawn the debounced hook, returning the shared
/// handle used by the open/close hooks.
fn spawn() -> Option<WatcherHandle> {
    let handle_slot: Arc<Mutex<Option<WatcherHandle>>> = Arc::new(Mutex::new(None));

    let tx: Sender<WatchEvent> = FileWatcherHandler {
        pending: HashSet::new(),
        handle: handle_slot.clone(),
    }
    .spawn();

    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else {
            return;
        };
        // Only care about content/metadata mutations; ignore access events.
        if !matches!(
            event.kind,
            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any
        ) {
            return;
        }
        if event.paths.is_empty() {
            return;
        }
        send_blocking(&tx, WatchEvent::Changed(event.paths));
    })
    .ok()?;

    let handle = WatcherHandle {
        inner: Arc::new(Mutex::new(WatcherState {
            watcher,
            files: HashSet::new(),
            dirs: HashMap::new(),
        })),
    };
    // Late-bind the handle into the hook so its debounce can filter events.
    *handle_slot.lock().unwrap() = Some(handle.clone());
    Some(handle)
}

/// Register the file watcher. Called once from `events::register()`.
///
/// No-op if the watcher backend fails to initialize, so the editor still runs.
pub fn register() {
    let Some(handle) = spawn() else {
        log::warn!("file-watcher: failed to initialize; auto-reload disabled");
        return;
    };

    let open_handle = handle.clone();
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        if let Some(path) = event
            .editor
            .document(event.doc)
            .and_then(|doc| doc.path().cloned())
        {
            open_handle.watch(&path);
        }
        Ok(())
    });

    let close_handle = handle;
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        if let Some(path) = event.doc.path() {
            close_handle.unwatch(path);
        }
        Ok(())
    });
}
