//! The machine-readable command tree of this command line, printed for
//! `__command-tree` and tracked as `cli/command-tree.json`, so that
//! documentation checks can confirm that shown commands and flags exist
//! without building Blobray. A node lists its path from `blobray`, its
//! visible subcommands and long flags, and whether it forwards its trailing
//! arguments to another program.

use clap::CommandFactory as _;

/// The only argument with which Blobray prints its command tree.
pub const REQUEST: &str = "__command-tree";

/// Every visible command of the command line, depth first, as a JSON array.
pub fn json() -> String {
    let mut nodes = Vec::new();
    walk(
        &super::Cli::command(),
        vec![String::from("blobray")],
        &mut nodes,
    );
    let mut text = serde_json::to_string_pretty(&nodes).expect("a command tree serializes");
    text.push('\n');
    text
}

fn walk(command: &clap::Command, path: Vec<String>, nodes: &mut Vec<serde_json::Value>) {
    let visible = command
        .get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set())
        .collect::<Vec<_>>();
    let flags = command
        .get_arguments()
        .filter(|argument| !argument.is_hide_set())
        .flat_map(|argument| {
            argument
                .get_long()
                .into_iter()
                .chain(argument.get_visible_aliases().unwrap_or_default())
                .map(|long| format!("--{long}"))
        })
        .collect::<Vec<_>>();
    let forwards = command.get_positionals().any(|argument| {
        argument.is_trailing_var_arg_set() && argument.is_allow_hyphen_values_set()
    });
    nodes.push(serde_json::json!({
        "path": path,
        "subcommands": visible.iter().map(|s| s.get_name()).collect::<Vec<_>>(),
        "flags": flags,
        "forwards": forwards,
    }));
    for subcommand in visible {
        let mut child = path.clone();
        child.push(subcommand.get_name().to_owned());
        walk(subcommand, child, nodes);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_tree_names_commands_and_global_flags() {
        let nodes: Vec<serde_json::Value> = serde_json::from_str(&super::json()).unwrap();
        assert_eq!(nodes[0]["path"], serde_json::json!(["blobray"]));
        assert!(
            nodes[0]["flags"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("--format"))
        );
        let audit = nodes
            .iter()
            .find(|node| node["path"] == serde_json::json!(["blobray", "audit-targets"]))
            .unwrap();
        assert!(
            audit["flags"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("--forbid"))
        );
        assert!(nodes.iter().all(|node| node["forwards"] == false));
    }
}
