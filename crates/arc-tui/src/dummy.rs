//! Dummy data for development/testing. Enable with DUMMY_DATA=1 env var.

use arc_core::time;

use crate::api::{
    iteration::{Iteration, IterationStatus},
    story::Story,
};

pub fn is_enabled() -> bool {
    std::env::var("DUMMY_DATA").is_ok_and(|val| val == "1")
}

pub fn iteration() -> Iteration {
    let today = time::today();
    Iteration {
        id: (-1).into(),
        name: "Sprint 42".to_string(),
        start_date: today - chrono::Duration::days(7),
        end_date: today + chrono::Duration::days(7),
        app_url: "https://app.shortcut.com/example/iteration/1".to_string(),
        status: IterationStatus::Started,
    }
}

pub fn stories() -> Vec<Story> {
    vec![
        Story {
            id: 101.into(),
            name: "Implement user authentication".to_string(),
            description: "Add login/logout functionality with OAuth2.\n\nAcceptance criteria:\n- Users can log in with Google\n- Session persists across browser refresh\n- Logout clears all tokens".to_string(),
            completed: false,
            branches: vec![],
            comments: vec![],
            epic_id: Some(10.into()),
            iteration_id: Some(1.into()),
            app_url: "https://app.shortcut.com/example/story/101".to_string(),
            ..Default::default()
        },
        Story {
            id: 102.into(),
            name: "Fix pagination bug on search results".to_string(),
            description: "When there are more than 100 results, the pagination breaks and shows duplicate items on page 2.".to_string(),
            completed: false,
            branches: vec![],
            comments: vec![],
            epic_id: None,
            iteration_id: Some(1.into()),
            app_url: "https://app.shortcut.com/example/story/102".to_string(),
            ..Default::default()
        },
        Story {
            id: 103.into(),
            name: "Add dark mode support".to_string(),
            description: "Implement system-aware dark mode with manual toggle.\n\nDesign specs in Figma.".to_string(),
            completed: false,
            branches: vec![],
            comments: vec![],
            epic_id: Some(10.into()),
            iteration_id: Some(1.into()),
            app_url: "https://app.shortcut.com/example/story/103".to_string(),
            ..Default::default()
        },
        Story {
            id: 104.into(),
            name: "Refactor database connection pooling".to_string(),
            description: "Current implementation creates new connections for each request. Switch to connection pooling with configurable limits.\n\nBenchmark before/after.".to_string(),
            completed: false,
            branches: vec![],
            comments: vec![],
            epic_id: Some(20.into()),
            iteration_id: Some(1.into()),
            app_url: "https://app.shortcut.com/example/story/104".to_string(),
            ..Default::default()
        },
        Story {
            id: 105.into(),
            name: "Write API documentation".to_string(),
            description: "Document all public endpoints with examples.".to_string(),
            completed: false,
            branches: vec![],
            comments: vec![],
            epic_id: None,
            iteration_id: Some(1.into()),
            app_url: "https://app.shortcut.com/example/story/105".to_string(),
            ..Default::default()
        },
    ]
}
