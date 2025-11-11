use clap::{Parser, Subcommand};
use libp2p::{
    gossipsub, identity, mdns, noise, tcp, yamux, PeerId,
    kad::{self, store::MemoryStore},
    swarm::{NetworkBehaviour, SwarmEvent},
    futures::StreamExt,
    request_response::{self, ProtocolSupport},
    StreamProtocol,
};
use std::error::Error;
use std::path::PathBuf;
use tokio::io::{self, AsyncBufReadExt};
use tracing_subscriber::EnvFilter;
use serde::{Serialize, Deserialize};
use tokio::fs;
use sha2::{Sha256, Digest};

#[derive(Parser)]
#[command(name = "p2p-fileshare")]
#[command(about = "Decentralized P2P file sharing", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Start {
        #[arg(short, long, default_value = "0")]
        port: u16,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileRequest {
    filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileResponse {
    filename: String,
    data: Vec<u8>,
    hash: String,
}

#[derive(NetworkBehaviour)]
struct P2PBehaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    request_response: request_response::cbor::Behaviour<FileRequest, FileResponse>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start { port } => start_node(port).await?,
    }

    Ok(())
}

async fn start_node(port: u16) -> Result<(), Box<dyn Error>> {
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    
    tracing::info!("Local peer id: {local_peer_id}");

    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(std::time::Duration::from_secs(1))
        .validation_mode(gossipsub::ValidationMode::Permissive)
        .mesh_n_low(0)
        .mesh_n(1)
        .mesh_n_high(2)
        .mesh_outbound_min(0)
        .flood_publish(true)
        .build()
        .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))?;

    let gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )?;

    let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)?;

    let store = MemoryStore::new(local_peer_id);
    let kademlia = kad::Behaviour::new(local_peer_id, store);

    let request_response = request_response::cbor::Behaviour::new(
        [(StreamProtocol::new("/file-exchange/1.0.0"), ProtocolSupport::Full)],
        request_response::Config::default(),
    );

    let behaviour = P2PBehaviour {
        gossipsub,
        mdns,
        kademlia,
        request_response,
    };

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|_| behaviour)?
        .with_swarm_config(|c| c.with_idle_connection_timeout(std::time::Duration::from_secs(60)))
        .build();

    let listen_addr = format!("/ip4/0.0.0.0/tcp/{}", if port == 0 { 0 } else { port });
    swarm.listen_on(listen_addr.parse()?)?;

    let topic = gossipsub::IdentTopic::new("file-share");
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    let mut stdin = io::BufReader::new(io::stdin()).lines();

    tracing::info!("Node started. Commands:");
    tracing::info!("  send <filepath> <peer_id>");
    tracing::info!("  list");
    tracing::info!("  Or type any message to broadcast");

    let mut peers: Vec<PeerId> = Vec::new();

    loop {
        tokio::select! {
            Ok(Some(line)) = stdin.next_line() => {
                let parts: Vec<&str> = line.trim().split_whitespace().collect();
                
                if parts.is_empty() {
                    continue;
                }

                match parts[0] {
                    "send" if parts.len() >= 3 => {
                        let filepath = parts[1];
                        let peer_str = parts[2];
                        
                        match peer_str.parse::<PeerId>() {
                            Ok(_peer_id) => {
                                match fs::read(filepath).await {
                                    Ok(data) => {
                                        let mut hasher = Sha256::new();
                                        hasher.update(&data);
                                        let hash = format!("{:x}", hasher.finalize());
                                        
                                        let response = FileResponse {
                                            filename: PathBuf::from(filepath)
                                                .file_name()
                                                .unwrap()
                                                .to_string_lossy()
                                                .to_string(),
                                            data,
                                            hash,
                                        };
                                        
                                        let msg = serde_json::to_string(&response).unwrap();
                                        if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), msg.as_bytes()) {
                                            tracing::error!("Failed to send file: {e:?}");
                                        } else {
                                            tracing::info!("File sent via gossipsub");
                                        }
                                    }
                                    Err(e) => tracing::error!("Failed to read file: {e}"),
                                }
                            }
                            Err(e) => tracing::error!("Invalid peer ID: {e}"),
                        }
                    }
                    "list" => {
                        tracing::info!("Connected peers:");
                        for peer in &peers {
                            tracing::info!("  {peer}");
                        }
                    }
                    _ => {
                        if let Err(e) = swarm
                            .behaviour_mut()
                            .gossipsub
                            .publish(topic.clone(), line.as_bytes())
                        {
                            tracing::error!("Publish error: {e:?}");
                        }
                    }
                }
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => {
                    tracing::info!("Listening on {address}");
                }
                SwarmEvent::Behaviour(P2PBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, multiaddr) in list {
                        tracing::info!("Discovered peer: {peer_id}");
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                        swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr);
                        if !peers.contains(&peer_id) {
                            peers.push(peer_id);
                        }
                    }
                }
                SwarmEvent::Behaviour(P2PBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, _multiaddr) in list {
                        tracing::info!("Peer expired: {peer_id}");
                        swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                        peers.retain(|p| p != &peer_id);
                    }
                }
                SwarmEvent::Behaviour(P2PBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message,
                    ..
                })) => {
                    if let Ok(file_resp) = serde_json::from_slice::<FileResponse>(&message.data) {
                        tracing::info!("Receiving file: {} ({} bytes)", file_resp.filename, file_resp.data.len());
                        
                        let mut hasher = Sha256::new();
                        hasher.update(&file_resp.data);
                        let computed_hash = format!("{:x}", hasher.finalize());
                        
                        if computed_hash == file_resp.hash {
                            let save_path = PathBuf::from("downloads").join(&file_resp.filename);
                            fs::create_dir_all("downloads").await.ok();
                            
                            match fs::write(&save_path, &file_resp.data).await {
                                Ok(_) => tracing::info!("File saved to: {}", save_path.display()),
                                Err(e) => tracing::error!("Failed to save file: {e}"),
                            }
                        } else {
                            tracing::error!("File hash mismatch!");
                        }
                    } else {
                        tracing::info!(
                            "Message from {peer_id}: {}",
                            String::from_utf8_lossy(&message.data)
                        );
                    }
                }
                _ => {}
            }
        }
    }
}
