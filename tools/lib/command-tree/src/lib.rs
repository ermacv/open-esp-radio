//! The machine-readable command tree of a command line, which every
//! repository tool prints for `__command-tree`: the commands and flags that
//! exist. With the `clap` feature, [`command_tree`] walks a clap parser.

use serde::{Deserialize, Serialize};

/// One command of a tree: its path from the root, its subcommands and the
/// long flags it accepts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandNode {
    pub path: Vec<String>,
    pub subcommands: Vec<String>,
    pub flags: Vec<String>,
    /// The command passes its trailing arguments, flags included, to another
    /// program, so they are not its own.
    #[serde(default)]
    pub forwards: bool,
}

/// The only argument with which a tool prints its command tree instead of
/// running.
pub const REQUEST: &str = "__command-tree";

/// Whether this process was started with [`REQUEST`] alone.
pub fn requested() -> bool {
    let mut arguments = std::env::args_os().skip(1);
    arguments.next().is_some_and(|argument| argument == REQUEST) && arguments.next().is_none()
}

/// The tree as the JSON array the documentation check reads.
pub fn json(nodes: &[CommandNode]) -> String {
    serde_json::to_string_pretty(nodes).expect("a command tree serializes")
}

/// Every command of `command` below `prefix`, depth first. Hidden commands
/// and arguments are left out.
#[cfg(feature = "clap")]
pub fn command_tree(command: &clap::Command, prefix: &[String]) -> Vec<CommandNode> {
    let mut nodes = Vec::new();
    walk(command, prefix.to_vec(), &mut nodes);
    nodes
}

#[cfg(feature = "clap")]
fn walk(command: &clap::Command, path: Vec<String>, nodes: &mut Vec<CommandNode>) {
    let visible = command
        .get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set())
        .collect::<Vec<_>>();
    nodes.push(CommandNode {
        path: path.clone(),
        subcommands: visible
            .iter()
            .map(|subcommand| subcommand.get_name().to_owned())
            .collect(),
        flags: command
            .get_arguments()
            .filter(|argument| !argument.is_hide_set())
            .flat_map(|argument| {
                argument
                    .get_long()
                    .into_iter()
                    .chain(argument.get_visible_aliases().unwrap_or_default())
                    .map(|long| format!("--{long}"))
            })
            .collect(),
        forwards: command.get_positionals().any(|argument| {
            argument.is_trailing_var_arg_set() && argument.is_allow_hyphen_values_set()
        }),
    });
    for subcommand in visible {
        let mut child = path.clone();
        child.push(subcommand.get_name().to_owned());
        walk(subcommand, child, nodes);
    }
}

#[cfg(all(test, feature = "clap"))]
mod tests {
    use super::*;

    #[test]
    fn a_tree_names_every_visible_command_and_long_flag() {
        let command = clap::Command::new("root")
            .arg(
                clap::Arg::new("json")
                    .long("json")
                    .visible_alias("machine")
                    .action(clap::ArgAction::SetTrue),
            )
            .subcommand(
                clap::Command::new("list")
                    .arg(clap::Arg::new("since").long("since"))
                    .arg(clap::Arg::new("secret").long("secret").hide(true)),
            )
            .subcommand(clap::Command::new("__hidden").hide(true))
            .subcommand(
                clap::Command::new("forward").arg(
                    clap::Arg::new("args")
                        .num_args(0..)
                        .trailing_var_arg(true)
                        .allow_hyphen_values(true),
                ),
            );
        let tree = command_tree(&command, &[String::from("hil")]);
        assert_eq!(tree.len(), 3);
        assert_eq!(tree[0].path, ["hil"]);
        assert_eq!(tree[0].subcommands, ["list", "forward"]);
        assert!(tree[0].flags.contains(&String::from("--json")));
        assert!(tree[0].flags.contains(&String::from("--machine")));
        assert_eq!(tree[1].path, ["hil", "list"]);
        assert_eq!(tree[1].flags, ["--since"]);
        assert!(!tree[1].forwards);
        assert!(tree[2].forwards);
    }
}
