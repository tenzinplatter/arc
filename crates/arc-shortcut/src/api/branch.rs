use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, Debug)]
pub struct Branch {
    id: i32,
    name: String,
}

impl Branch {
    pub fn name(&self) -> &str {
        &self.name
    }
}
