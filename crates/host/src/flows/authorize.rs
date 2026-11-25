//! 谁能做流程里人的决定（design/flows.md §4–§5）：确认、放行搁置的站、强制放行只认人，任何工具调用都做不了——
//! 会话里的模型不论在什么权限模式下都不行；确认写了角色的，按项目的 `roles.yaml` 核对是不是这个人；走外部审批的
//! 确认只能由审批系统决定。
//!
//! 实例的 socket 是 0600，接得进来的只有实例的主人，所以"人"就是这个操作系统用户：TUI 做的操作记成
//! `Actor{kind: "person", id: <用户名>}`（[`person`]）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use harnix_flow::{Actor, Authorizer, ConfirmVia, ExitCond, Op, RunRecord};

/// 人亲手做的操作的 `Actor.kind`。
pub const PERSON_KIND: &str = "person";

/// 这台机器上接着实例的人：实例的主人。
pub fn person() -> Actor {
    Actor::new(PERSON_KIND, os_user())
}

fn os_user() -> String {
    let uid = nix::unistd::geteuid();
    nix::unistd::User::from_uid(uid)
        .ok()
        .flatten()
        .map(|u| u.name)
        .unwrap_or_else(|| uid.to_string())
}

/// 项目的 `.conix/flows/roles.yaml`：角色 → 能确认的操作系统用户名。进 git，改它要走代码评审；
/// 读不到、读不懂就当没有（角色没列出来，任何人都行）。
pub struct ConixAuthorizer {
    pub roles_file: PathBuf,
}

impl ConixAuthorizer {
    fn roles(&self) -> BTreeMap<String, Vec<String>> {
        let Ok(text) = std::fs::read_to_string(&self.roles_file) else {
            return BTreeMap::new();
        };
        serde_yaml::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!(file = %self.roles_file.display(), error = %e, "roles not read");
            BTreeMap::new()
        })
    }
}

impl Authorizer for ConixAuthorizer {
    fn check(&self, op: &Op, by: &Actor, run: Option<&RunRecord>) -> Result<(), String> {
        let what = match op {
            Op::Confirm { .. } => "confirming a station",
            Op::Force { .. } => "forcing a station past its conditions",
            Op::Release { .. } => "releasing a held station",
            _ => return Ok(()),
        };
        if by.kind != PERSON_KIND {
            return Err(format!(
                "{what} is a person's decision and no tool call makes it: ask the user to do it in Conix's Flows tab"
            ));
        }
        let Op::Confirm { station, who, .. } = op else {
            return Ok(());
        };
        if let Some(server) = run.and_then(|r| mcp_server(r, station)) {
            return Err(format!(
                "station {station} is decided in {server}'s approval, not here: it is recorded once {server} decides"
            ));
        }
        let roles = self.roles();
        let listed = who
            .as_ref()
            .and_then(|role| roles.get(role).map(|p| (role, p)))
            .filter(|(_, p)| !p.is_empty());
        match listed {
            Some((role, people)) if !people.contains(&by.id) => Err(format!(
                "station {station} is confirmed by {role} ({}), and you are {}: it needs their approval — a flow asks them with `via: mcp`",
                people.join(", "),
                by.id
            )),
            _ => Ok(()),
        }
    }
}

/// 这一站的确认走外部审批时，是哪个服务。
fn mcp_server(rec: &RunRecord, station: &str) -> Option<String> {
    rec.def.station(station)?.exit.iter().find_map(|e| match e {
        ExitCond::Confirm {
            via: Some(ConfirmVia::Mcp),
            mcp,
            ..
        } => Some(
            mcp.as_ref()
                .map_or_else(|| "the outside".to_string(), |m| m.server.clone()),
        ),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(roles: &str) -> (tempfile::TempDir, ConixAuthorizer) {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("roles.yaml");
        std::fs::write(&f, roles).unwrap();
        (d, ConixAuthorizer { roles_file: f })
    }

    fn confirm(who: Option<&str>) -> Op {
        Op::Confirm {
            station: "review".into(),
            pass: true,
            who: who.map(String::from),
        }
    }

    #[test]
    fn only_a_person_confirms_forces_or_releases() {
        let (_d, a) = auth("");
        let model = Actor::new(harnix_flow::author::CONVERSATION_KIND, "s1");
        for op in [
            confirm(None),
            Op::Force {
                station: "x".into(),
            },
            Op::Release {
                station: "x".into(),
            },
        ] {
            let e = a.check(&op, &model, None).unwrap_err();
            assert!(e.contains("no tool call makes it"), "{e}");
            assert!(a.check(&op, &Actor::agent("s2"), None).is_err());
            assert!(
                a.check(&op, &Actor::new(PERSON_KIND, "alice"), None)
                    .is_ok()
            );
        }
        // The rest is not a person's alone.
        assert!(a.check(&Op::Pause, &model, None).is_ok());
    }

    #[test]
    fn a_role_is_confirmed_by_the_people_listed_for_it() {
        let (_d, a) = auth("reviewer: [alice, bob]\n");
        assert!(
            a.check(
                &confirm(Some("reviewer")),
                &Actor::new(PERSON_KIND, "bob"),
                None
            )
            .is_ok()
        );
        let e = a
            .check(
                &confirm(Some("reviewer")),
                &Actor::new(PERSON_KIND, "carol"),
                None,
            )
            .unwrap_err();
        assert!(
            e.contains("reviewer (alice, bob)") && e.contains("via: mcp"),
            "{e}"
        );
        // A role no one is listed for: anyone.
        assert!(
            a.check(
                &confirm(Some("owner")),
                &Actor::new(PERSON_KIND, "carol"),
                None
            )
            .is_ok()
        );
    }
}
