//! Snapshot units keep the dependencies read by their own loaders. No polling
//! thread, disk cache, generation counter or new on-disk state.
use super::*;
use crate::bbs_sync::{control::Membership, SyncState};

struct Inputs {
    sync_state: SyncState,
    membership: Option<Membership>,
    source_deps: cache::Deps,
    state: BbsProjectState,
    thread_ids: Vec<String>,
}
struct PostSide {
    sync_avatar: Option<sync::AvatarView>,
    agent_avatar: Option<String>,
    attachments: Vec<BbsAttachmentView>,
}
struct Sides {
    posts: BTreeMap<(String, String), PostSide>,
    unavailable: Vec<sync::UnavailableView>,
}
type ProjectKey = (PathBuf, PathBuf, String);
static INPUTS: cache::Memo<ProjectKey, Inputs> = cache::Memo::new(8, 8 * 1024 * 1024);
static THREADS: cache::Memo<(PathBuf, String), (BbsThreadMeta, Vec<LoadedPost>)> =
    cache::Memo::new(256, 24 * 1024 * 1024);
static SIDES: cache::Memo<(ProjectKey, String), Sides> = cache::Memo::new(512, 8 * 1024 * 1024);

#[cfg(test)]
mod tests;

pub(super) fn load(
    content: &sync::ContentStore,
    project_id: &str,
    project_display_name: Option<&str>,
) -> Result<BbsSnapshot> {
    let root = content.root().to_path_buf();
    let key = (
        root.clone(),
        content.account_dir().to_path_buf(),
        project_id.to_owned(),
    );
    let inputs = INPUTS.get(key.clone(), |deps| {
        // Attachment projection depends on sync authority, not on this
        // project's read/ignored flags or the board's thread enumeration.
        let mut source_deps = cache::Deps::new();
        let sync_state = content
            .state
            .load_state_tracked(&mut source_deps)
            .map_err(anyhow::Error::msg)?
            .unwrap_or_default();
        let membership =
            crate::bbs_sync::control::read_membership_tracked(&content.state, &mut source_deps)
                .map_err(anyhow::Error::msg)?;
        deps.inherit_value(&source_deps);
        Ok(Inputs {
            sync_state,
            membership,
            source_deps,
            state: load_project_state_tracked(&root, project_id, deps)?,
            thread_ids: read_thread_ids_tracked(&root, deps)?,
        })
    })?;
    let Inputs {
        sync_state,
        membership,
        source_deps: _,
        state,
        thread_ids,
    } = inputs.value.as_ref();
    let mut threads = Vec::new();
    let mut new_count = 0usize;
    for thread_id in thread_ids {
        let Ok(thread) = THREADS.get((root.clone(), thread_id.clone()), |deps| {
            load_thread_tracked(&root, thread_id, deps)
        }) else {
            continue;
        };
        let (meta, posts) = thread.value.as_ref();
        let mut posts = posts.clone();
        let mut seed = cache::Deps::new();
        seed.inherit(&inputs.value.source_deps);
        seed.inherit(&thread.deps);
        let sides = SIDES.get_seeded((key.clone(), thread_id.clone()), &seed, |deps| {
            let mut avatar_cache = BTreeMap::new();
            let mut posts = BTreeMap::new();
            let live = thread.value.1.iter().filter(|p| {
                !crate::bbs_sync::reconcile::suppressed(
                    sync_state,
                    thread_id,
                    &p.meta.post_id,
                    &p.version_id,
                )
            });
            let real_posts = live.clone().map(|p| p.meta.post_id.clone()).collect();
            let unavailable = content
                .unavailable_views_tracked(thread_id, sync_state, &real_posts, deps)
                .unwrap_or_default();
            for post in live {
                let sync_avatar = content.avatar_view_tracked(
                    thread_id,
                    &post.meta.post_id,
                    &post.version_id,
                    Some(sync_state),
                    deps,
                );
                let agent_avatar = if sync_avatar.is_none() {
                    post_agent_avatar_tracked(&root, &post.meta, &mut avatar_cache, deps)
                } else {
                    None
                };
                let attachments = content.attachment_views_tracked(
                    post,
                    Some(sync_state),
                    deps,
                );
                posts.insert(
                    (post.meta.post_id.clone(), post.version_id.clone()),
                    PostSide {
                        sync_avatar,
                        agent_avatar,
                        attachments,
                    },
                );
            }
            Ok(Sides { posts, unavailable })
        })?;
        posts.retain(|p| {
            !crate::bbs_sync::reconcile::suppressed(
                sync_state,
                thread_id,
                &p.meta.post_id,
                &p.version_id,
            )
        });
        let unavailable_posts = sides.value.unavailable.clone();
        if !posts.iter().any(|p| p.meta.kind == "topic") && unavailable_posts.is_empty() {
            continue;
        }
        let relevant = thread_relevant(meta, &posts, project_id);
        let mut post_views = Vec::new();
        let mut thread_is_new = false;
        for post in posts {
            let external = post.meta.project_id != project_id;
            let state_label = if state.processed_posts.contains_key(&post.meta.post_id) {
                "processed"
            } else if state.ignored_posts.contains_key(&post.meta.post_id) {
                "ignored"
            } else if relevant && external {
                thread_is_new = true;
                "new"
            } else {
                "none"
            };
            let preview_body = if post.body.trim().is_empty() {
                post.meta
                    .attachments
                    .iter()
                    .map(|item| item.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                post.body.clone()
            };
            let side = &sides.value.posts[&(post.meta.post_id.clone(), post.version_id.clone())];
            post_views.push(BbsPostView {
                post_id: post.meta.post_id.clone(),
                version_id: post.version_id.clone(),
                sync_avatar: side.sync_avatar.clone(),
                thread_id: post.meta.thread_id.clone(),
                project_id: post.meta.project_id.clone(),
                project_display_name: display_project_name_with_fallback(
                    &post.meta.project_id,
                    Some(&post.meta.project_display_name),
                ),
                agent_id: post.meta.agent_id.clone(),
                agent_display_name: post.meta.agent_display_name.clone(),
                agent_avatar: side.agent_avatar.clone(),
                created_at: post.meta.created_at.clone(),
                kind: post.meta.kind.clone(),
                preview: preview_markdown(&preview_body),
                attachments: {
                    let mut views = side.attachments.clone();
                    sync::apply_paused(&post.meta.post_id, &post.version_id, &mut views);
                    views
                },
                body: post.body,
                state: state_label.into(),
                external,
            });
        }
        if thread_is_new {
            new_count += 1;
        }
        threads.push(BbsThreadView {
            thread_id: meta.thread_id.clone(),
            visibility: meta.visibility.clone(),
            project_tag_labels: meta
                .project_tags
                .iter()
                .map(|id| display_project_name_with_fallback(id, None))
                .collect(),
            project_tags: meta.project_tags.clone(),
            created_by_project_label: display_project_name_with_fallback(
                &meta.created_by_project,
                None,
            ),
            created_by_project: meta.created_by_project.clone(),
            updated_at: meta.updated_at.clone(),
            latest_post_id: meta.latest_post_id.clone(),
            is_new: thread_is_new,
            relevant,
            sharing_group_id: membership
                .as_ref()
                .filter(|m| {
                    sync_state
                        .groups
                        .get(&m.group_id)
                        .is_some_and(|g| g.shared_threads.contains(thread_id))
                })
                .map(|m| m.group_id.clone()),
            posts: post_views,
            unavailable_posts,
        });
    }
    threads.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(BbsSnapshot {
        project_id: project_id.to_string(),
        project_display_name: display_project_name_with_fallback(project_id, project_display_name),
        root: root.display().to_string(),
        new_count,
        threads,
    })
}
