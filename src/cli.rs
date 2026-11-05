//! Host command transport; production composition is system-only.
use std::{
    env,
    error::Error,
    fs,
    io::{self, Read},
    process::ExitCode,
};

use crate::{Packet, graph::Graph, node::Registry, runtime::Engine};

const HELP: &str = "DIY_IME2 core prototype\n\nCommands:\n  diy-ime2 validate-packet <file|->\n  diy-ime2 validate-graph <file|->\n  diy-ime2 run <graph-file|->\n  diy-ime2 list-nodes\n  diy-ime2 session\n  diy-ime2 session-v3 <resource-policy-file>\n  diy-ime2 broker-v3 <resource-policy-file> [owner-lock-file]\n\nThe production registry contains only four Host system adapters. Use diy-ime2-app and approved external Wasm packages for processing graphs.\nUse - to read UTF-8 JSON from standard input. session uses JSON Lines protocol v2.\nsession-v3 requires a complete explicit policy file and uses a standalone protocol-v3 Host. broker-v3 multiplexes Sessions on one shared Host.\nOptional DIY_IME_RESOURCE_ROOT selects one absolute local directory at process startup; unset denies resource access.\nResource references use /, and reparse links are rejected.";

fn read(path: &str) -> io::Result<String> {
    if path == "-" {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        Ok(input)
    } else {
        fs::read_to_string(path)
    }
}

fn execute(args: &[String], registry: Registry) -> Result<bool, Box<dyn Error>> {
    if args.is_empty() || (args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h")) {
        println!("{HELP}");
        return Ok(true);
    }
    let resource_root = if args.first().is_some_and(|command| {
        matches!(
            command.as_str(),
            "session" | "session-v3" | "broker-v3" | "run" | "validate-graph"
        )
    }) {
        crate::resources::ResourceRoot::from_env()?
    } else {
        None
    };
    if args == ["session"] {
        crate::session::serve_session(
            crate::session::Session::with_resource_root(registry, resource_root),
            io::stdin().lock(),
            io::stdout().lock(),
        )?;
        return Ok(true);
    }
    match args {
        [command] if command == "list-nodes" => {
            let definitions: Vec<_> = registry.definitions().collect();
            println!("{}", serde_json::to_string_pretty(&definitions)?);
            Ok(true)
        }
        [command, path] if command == "validate-packet" => {
            let packet = Packet::from_json(&read(path)?)?;
            println!("{}", packet.to_json()?);
            Ok(true)
        }
        [command, path, ownership] if command == "broker-v3" && path != "-" => {
            let value = serde_json::from_str(&fs::read_to_string(path)?)?;
            let policy = crate::execution_control::ResourcePolicy::from_value(value)?;
            crate::execution_control::broker::serve_host_with_resource_root(
                policy,
                registry,
                io::stdin().lock(),
                io::stdout().lock(),
                Some(std::path::Path::new(ownership)),
                resource_root,
            )?;
            Ok(true)
        }
        [command, path] if command == "broker-v3" && path != "-" => {
            let value = serde_json::from_str(&fs::read_to_string(path)?)?;
            let policy = crate::execution_control::ResourcePolicy::from_value(value)?;
            crate::execution_control::broker::serve_host_with_resource_root(
                policy,
                registry,
                io::stdin().lock(),
                io::stdout().lock(),
                None,
                resource_root,
            )?;
            Ok(true)
        }
        [command, path] if command == "session-v3" && path != "-" => {
            let value = serde_json::from_str(&fs::read_to_string(path)?)?;
            let policy = crate::execution_control::ResourcePolicy::from_value(value)?;
            crate::execution_control::stream::serve_host_with_resource_root(
                policy,
                registry,
                io::stdin().lock(),
                io::stdout().lock(),
                resource_root,
            )?;
            Ok(true)
        }
        [command, path] if command == "run" || command == "validate-graph" => {
            let graph = Graph::from_json(&read(path)?)?;
            let plan = crate::resources::compile(&graph, &registry, resource_root.as_deref())?;
            if command == "validate-graph" {
                let order: Vec<_> = plan.node_order().collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({"valid":true,"order":order}))?
                );
                Ok(true)
            } else {
                let report = Engine::new(plan).run()?;
                println!("{}", report.to_json()?);
                Ok(report.succeeded())
            }
        }
        _ => Err(format!("invalid command or arguments\n\n{HELP}").into()),
    }
}

/// Run the CLI with an explicitly composed trusted registry.
pub fn run(registry: Registry) -> ExitCode {
    match execute(&env::args().skip(1).collect::<Vec<_>>(), registry) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
