pub mod builtins;
pub mod middleware;
pub mod protocol;
pub mod registry;

pub use middleware::SubAgentMiddleware;
pub use protocol::{
    filter_state_for_child, merge_child_state, CompiledSubAgent, SubAgentInfo, SubAgentRegistry,
    SubAgentRunOutput, SubAgentRunRequest, TaskInput, EXCLUDED_STATE_KEYS,
};
pub use registry::InMemorySubAgentRegistry;
