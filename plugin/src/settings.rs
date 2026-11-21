use bmcbl_plugin_api::ActionEvent;
use bmcbl_plugin_api::prelude::*;

use super::Config;
use super::page::{detail, label, page_body, section, text};

pub(super) const PAGE_ID: &str = "room-settings";

#[derive(Default)]
pub(super) struct Settings {
    player_name: String,
    game_port: String,
    bootstrap_peers: String,
    disable_p2p: bool,
    status: Option<Result<(), String>>,
}

impl From<&Config> for Settings {
    fn from(config: &Config) -> Self {
        Self {
            player_name: config.player_name.clone(),
            game_port: format_port(config.game_port),
            bootstrap_peers: config.bootstrap_peers.clone(),
            disable_p2p: config.disable_p2p,
            status: None,
        }
    }
}

impl Settings {
    pub(super) fn handle_action(&mut self, action: &ActionEvent, config: &mut Config) {
        let value = action.value.as_deref().unwrap_or_default();
        match action.action_id.as_str() {
            "player-name" | "player-name-submit" => self.player_name = value.to_owned(),
            "game-port" | "game-port-submit" => self.game_port = value.to_owned(),
            "bootstrap-peers" | "bootstrap-peers-submit" => self.bootstrap_peers = value.to_owned(),
            "disable-p2p" => self.disable_p2p = value == "true",
            "save" => {
                self.status = Some(self.save(config).map_err(|error| error.message));
                return;
            }
            _ => return,
        }
        self.status = None;
    }

    fn save(&self, config: &mut Config) -> PluginResult<()> {
        let port = game_port(&self.game_port)?;
        let player_name = self.player_name.trim();
        let peers = self.bootstrap_peers.trim();
        let mut document = read_config()?
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| plugin_error("config", error.to_string()))?;
        document["player_name"] = toml_edit::value(player_name);
        document["game_port"] = toml_edit::value(i64::from(port));
        document["bootstrap_peers"] = toml_edit::value(peers);
        document["disable_p2p"] = toml_edit::value(self.disable_p2p);
        write_config(document.to_string())?;
        config.game_port = port;
        config.player_name = player_name.to_owned();
        config.bootstrap_peers = peers.to_owned();
        config.disable_p2p = self.disable_p2p;
        Ok(())
    }

    pub(super) fn render(&self, config: &Config) -> ViewTree {
        // GPUI retains the edits in its inputs. Use the saved values as seeds so an
        // asynchronous render cannot send an earlier keystroke back to a focused input.
        let mut body = page_body()
            .child(detail("设置将在下次创建或加入房间时生效。"))
            .child(
                section("房间参数")
                    .child(label("玩家名"))
                    .child(View::input(
                        &config.player_name,
                        "留空时自动生成",
                        "player-name",
                    ))
                    .child(label("本机游戏端口"))
                    .child(View::input(
                        format_port(config.game_port),
                        "留空时自动探测",
                        "game-port",
                    )),
            )
            .child(View::node(ViewNode::Separator))
            .child(
                section("网络")
                    .child(label("引导节点"))
                    .child(View::input(
                        &config.bootstrap_peers,
                        "留空时使用公共节点",
                        "bootstrap-peers",
                    ))
                    .child(View::toggle("禁用 P2P", self.disable_p2p, "disable-p2p")),
            )
            .child(View::node(ViewNode::Separator))
            .child(
                View::row()
                    .padding(0)
                    .full_width()
                    .align(Align::Center)
                    .child(View::button("保存设置", "save")),
            );
        if let Some(status) = &self.status {
            body = body.child(match status {
                Ok(()) => detail("已保存。"),
                Err(message) => text(message, ThemeToken::Danger, false, true),
            });
        }
        body.finish()
    }
}

fn format_port(port: u16) -> String {
    if port == 0 {
        String::new()
    } else {
        port.to_string()
    }
}

fn game_port(raw: &str) -> PluginResult<u16> {
    match raw.trim() {
        "" | "0" => Ok(0),
        value => value
            .parse::<u16>()
            .ok()
            .filter(|port| *port >= 1025)
            .ok_or_else(|| plugin_error("game-port", "端口应为 1025–65535，留空时自动探测。")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_draft_preserves_saved_port_and_validates_edits() {
        let config = Config {
            game_port: 19132,
            ..Config::default()
        };
        let mut draft = Settings::from(&config);
        assert_eq!(draft.game_port, "19132");
        draft.game_port = " 65536 ".into();
        assert!(game_port(&draft.game_port).is_err());
        assert_eq!(game_port(" ").unwrap(), 0);
        assert_eq!(game_port(" 19133 ").unwrap(), 19133);
        assert_eq!(config.game_port, 19132);
    }
}
