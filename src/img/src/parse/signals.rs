//! STUB (agent A): structure and metadata signals.
use crate::model::{ParsedImage, Signal};

pub fn structure(p: &ParsedImage) -> Vec<Signal> {
    p.signals.clone()
}

pub fn metadata(_p: &ParsedImage) -> Vec<Signal> {
    vec![]
}
