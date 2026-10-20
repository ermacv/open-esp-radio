//! Forward parsed arguments using the destination's clap grammar.
use std::ffi::OsString;

/// Encode explicit values, omitting options already satisfied by fixed images.
/// The grammar supplies names, arity and flag actions; argv is never scanned.
pub(crate) fn encode(
    command: &clap::Command,
    matches: &clap::ArgMatches,
    omit: &[&str],
) -> Vec<OsString> {
    let mut args = Vec::new();
    let mut positional = Vec::new();
    for argument in command.get_arguments() {
        let id = argument.get_id().as_str();
        if omit.contains(&id)
            || matches.value_source(id) != Some(clap::parser::ValueSource::CommandLine)
        {
            continue;
        }
        if let Some(long) = argument.get_long() {
            match argument.get_action() {
                clap::ArgAction::SetTrue | clap::ArgAction::SetFalse => {
                    if matches.get_flag(id)
                        == matches!(argument.get_action(), clap::ArgAction::SetTrue)
                    {
                        args.push(format!("--{long}").into());
                    }
                }
                _ => {
                    if let Some(values) = matches.get_raw(id) {
                        for value in values {
                            if value.as_encoded_bytes().starts_with(b"-") {
                                let mut inline = OsString::from(format!("--{long}="));
                                inline.push(value);
                                args.push(inline);
                            } else {
                                args.extend([OsString::from(format!("--{long}")), value.into()]);
                            }
                        }
                    }
                }
            }
        } else if let Some(values) = matches.get_raw(id) {
            positional.extend(values.map(OsString::from));
        }
    }
    if positional
        .iter()
        .any(|value| value.as_encoded_bytes().starts_with(b"-"))
    {
        args.push("--".into());
    }
    args.extend(positional);
    if let Some((name, child)) = matches.subcommand()
        && let Some(grammar) = command.find_subcommand(name)
    {
        args.push(name.into());
        args.extend(encode(grammar, child, omit));
    }
    args
}
