use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use tokio::{sync::mpsc::{self, UnboundedSender}, task::JoinHandle};
use uuid::Uuid;

use crate::api::user::MemberId;

use crate::{
    api::ApiClient,
    app::{
        App,
        model::{DataState, Model, UiState},
        msg::Msg,
    },
    cache::Cache,
    dummy,
    error::ErrorInfo,
};
use arc_core::Config;

impl App {
    pub async fn init() -> Result<Self> {
        let config = Config::read()?;
        let mut cache = Cache::read(config.cache_dir.clone()).await;

        if dummy::is_enabled() {
            return Self::init_with_dummy_data(config, cache).await;
        }

        let api_client = ApiClient::from_token_and_cache(&config.api_token, &mut cache).await?;

        let todos = crate::todos::load_todos(&config.cache_dir).await;

        let (sender, receiver) = mpsc::unbounded_channel();
        let sender_clone = sender.clone();

        let key_reader_paused = Arc::new(AtomicBool::new(false));
        super::spawn_key_reader(sender.clone(), key_reader_paused.clone());

        spawn_todos_watcher(
            config.cache_dir.clone(),
            sender.clone(),
            tokio::runtime::Handle::current(),
        );

        let mut model = Model::from_cache_and_config(cache, config.clone(), todos);

        let handles = fetch_info_from_api(api_client.clone(), sender).await;
        model.data.async_handles.extend(handles);

        Ok(App {
            model,
            exit: false,
            receiver,
            sender: sender_clone,
            api_client,
            config,
            key_reader_paused,
        })
    }

    async fn init_with_dummy_data(config: Config, mut cache: Cache) -> Result<Self> {
        let dummy_user_id = MemberId::from(Uuid::nil());
        let dummy_mention_name = "dummy".to_string();
        let api_client = ApiClient::new(
            config.api_token.to_owned(),
            dummy_user_id,
            dummy_mention_name.clone(),
        );

        cache.user_id = Some(dummy_user_id);
        cache.user_mention_name = Some(dummy_mention_name);

        let (sender, receiver) = mpsc::unbounded_channel();
        let sender_clone = sender.clone();

        let key_reader_paused = Arc::new(AtomicBool::new(false));
        super::spawn_key_reader(sender.clone(), key_reader_paused.clone());

        let iteration = dummy::iteration();
        let stories = dummy::stories();

        let model = Model {
            data: DataState {
                stories: stories.clone(),
                epics: Vec::new(),
                current_iterations: Some(vec![iteration.clone()]),
                active_story: None,
                async_handles: Vec::new(),
                iterations: vec![iteration.clone()],
                todos: Vec::new(),
            },
            ui: UiState::default(),
            config: config.clone(),
            cache,
        };

        // Send messages so UI updates as if data loaded normally
        let _ = sender.send(Msg::IterationsLoaded(vec![iteration.clone()]));
        let _ = sender.send(Msg::AllIterationsLoaded(vec![iteration]));
        let _ = sender.send(Msg::StoriesLoaded {
            stories,
            from_cache: false,
        });

        Ok(App {
            model,
            exit: false,
            receiver,
            sender: sender_clone,
            api_client,
            config,
            key_reader_paused,
        })
    }
}

async fn fetch_info_from_api(api_client: ApiClient, sender: UnboundedSender<Msg>) -> Vec<JoinHandle<()>> {
    let iteration_client = api_client.clone();
    let iteration_sender = sender.clone();
    let current_iteration_handle = tokio::spawn(async move {
        match iteration_client.get_current_iterations().await {
            Ok(iterations) => {
                let _ = iteration_sender.send(Msg::IterationsLoaded(iterations));
            }
            Err(e) => {
                let info = ErrorInfo::new(
                    "Failed to fetch current iteration info".to_string(),
                    e.to_string(),
                );

                let _ = iteration_sender.send(Msg::Error(info));
            }
        };
    });

    let all_iter_client = api_client.clone();
    let all_iter_sender = sender.clone();

    let epics_handle = tokio::spawn(async move {
        match api_client.get_all_epics_slim(false).await {
            Ok(epics) => {
                let _ = sender.send(Msg::EpicsLoaded(epics));
            }
            Err(e) => {
                let info = ErrorInfo::new(
                    "Failed to fetch epics".to_string(),
                    e.to_string(),
                );
                let _ = sender.send(Msg::Error(info));
            }
        }
    });

    let all_iterations_handle = tokio::spawn(async move {
        match all_iter_client.get_all_iterations().await {
            Ok(iterations) => {
                let _ = all_iter_sender.send(Msg::AllIterationsLoaded(iterations));
            }
            Err(e) => {
                let info = ErrorInfo::new(
                    "Failed to fetch all iterations".to_string(),
                    e.to_string(),
                );
                let _ = all_iter_sender.send(Msg::Error(info));
            }
        }
    });

    vec![current_iteration_handle, epics_handle, all_iterations_handle]
}

fn spawn_todos_watcher(
    cache_dir: std::path::PathBuf,
    sender: UnboundedSender<Msg>,
    handle: tokio::runtime::Handle,
) {
    use notify::{EventKind, RecursiveMode, Watcher};
    use std::sync::mpsc as std_mpsc;

    std::thread::spawn(move || {
        let (tx, rx) = std_mpsc::channel();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(_) => return,
        };
        let path = cache_dir.join("todos.json");
        // Watch the cache_dir (not the file) — the file may not exist yet,
        // and atomic-rename writes replace the inode.
        let _ = watcher.watch(&cache_dir, RecursiveMode::NonRecursive);

        let mut last_emit = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap_or_else(std::time::Instant::now);

        for ev in rx.iter().flatten() {
            if !ev.paths.iter().any(|p| p == &path) {
                continue;
            }
            if !matches!(
                ev.kind,
                EventKind::Modify(_) | EventKind::Create(_)
            ) {
                continue;
            }
            // Debounce: at most one reload per 200ms.
            if last_emit.elapsed() < std::time::Duration::from_millis(200) {
                continue;
            }
            last_emit = std::time::Instant::now();

            let cache_dir = cache_dir.clone();
            let sender = sender.clone();
            handle.spawn(async move {
                let todos = crate::todos::load_todos(&cache_dir).await;
                let _ = sender.send(Msg::TodosReloaded(todos));
            });
        }
    });
}
