mod convert;
mod xml;

pub fn convert(
    config: &crate::config::Config,
    input: &std::path::Path,
    output: &std::path::Path,
    is_appending: bool,
    usage: &crate::common::usage::UsageBoost,
) -> Result<(), Box<dyn std::error::Error>> {
    convert::convert_all(config, input, output, is_appending, usage)
}
