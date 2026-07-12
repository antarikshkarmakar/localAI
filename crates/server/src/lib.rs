//! Main Brain server process (spec 01).
//! Coordinates: queue, workers, council, routing, memory.

pub mod heartbeat;
pub mod paths;
pub mod process_runner;
pub mod queue;
pub mod startup;
pub mod supervisor;
