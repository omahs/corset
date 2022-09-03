use crate::utils::ConstraintsSet;
use eyre::*;

mod generator;
mod parser;

pub fn compile(sources: &[(&str, &str)]) -> Result<ConstraintsSet> {
    generator::compile(sources)
}