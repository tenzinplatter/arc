use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! int_id {
    ($name:ident) => {
        #[derive(
            Copy,
            Clone,
            Debug,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(i32);

        impl $name {
            pub fn get(self) -> i32 {
                self.0
            }
        }

        impl From<i32> for $name {
            fn from(id: i32) -> Self {
                Self(id)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

int_id!(StoryId);
int_id!(IterationId);
int_id!(EpicId);
