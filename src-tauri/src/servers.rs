use std::{fs, path::Path};

use serde::Deserialize;

use crate::{error::AppResult, models::Server};

#[derive(Debug, Deserialize)]
struct InfoDocument {
    #[serde(default)]
    servers: ServerList,
}

#[derive(Debug, Default, Deserialize)]
struct ServerList {
    #[serde(rename = "server", default)]
    entries: Vec<XmlServer>,
}

#[derive(Debug, Deserialize)]
struct XmlServer {
    #[serde(rename = "@id")]
    id: String,
    name: String,
    ip: String,
    port: u16,
}

pub fn load_servers(game_dir: &Path) -> AppResult<Vec<Server>> {
    let path = game_dir.join("info.xml");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let xml = fs::read_to_string(path)?;
    parse_servers(&xml)
}

fn parse_servers(xml: &str) -> AppResult<Vec<Server>> {
    // The installed file contains sibling top-level elements; wrapping makes it one XML document.
    let wrapped = format!("<radmir>{xml}</radmir>");
    let document: InfoDocument = quick_xml::de::from_str(&wrapped)?;
    let mut servers: Vec<Server> = document
        .servers
        .entries
        .into_iter()
        .map(|entry| Server {
            id: entry.id,
            name: format!("SERVER {}", entry.name.trim_start_matches("SERVER ")),
            host: entry.ip,
            port: entry.port,
            online: None,
            players: None,
            capacity: Some(1000),
            bonus: None,
        })
        .collect();
    servers.sort_by_key(|server| server.id.parse::<u32>().unwrap_or(u32::MAX));
    servers.dedup_by(|left, right| left.id == right.id);
    Ok(servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_installed_info_xml_shape() {
        let xml = r#"
            <game>CR:MP</game>
            <servers>
                <server id="2"><name>SERVER 2</name><ip>127.0.0.2</ip><port>7778</port></server>
                <server id="1"><name>SERVER 1</name><ip>127.0.0.1</ip><port>7777</port></server>
            </servers>
        "#;
        let servers = parse_servers(xml).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].id, "1");
        assert_eq!(servers[0].host, "127.0.0.1");
        assert_eq!(servers[1].port, 7778);
        assert_eq!(servers[0].players, None);
    }
}
