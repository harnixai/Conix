//! 编进二进制的种子与场景：`conix --mock <名字>` 不依赖工作目录。

const SEEDS: &[(&str, &str)] = &[
    ("empty", include_str!("../seeds/empty.yaml")),
    ("default", include_str!("../seeds/default.yaml")),
    ("demo", include_str!("../seeds/demo.yaml")),
    ("xforge", include_str!("../seeds/xforge.yaml")),
];

const SCENARIOS: &[(&str, &str)] = &[
    ("demo", include_str!("../scenarios/demo.yaml")),
    ("default", include_str!("../scenarios/default.yaml")),
    ("m21-kicked", include_str!("../scenarios/m21-kicked.yaml")),
    (
        "m01-chat-basic",
        include_str!("../scenarios/m01-chat-basic.yaml"),
    ),
    (
        "m02-text-hard",
        include_str!("../scenarios/m02-text-hard.yaml"),
    ),
    ("m03-tools", include_str!("../scenarios/m03-tools.yaml")),
    ("m04-prompts", include_str!("../scenarios/m04-prompts.yaml")),
    (
        "m05-interrupt",
        include_str!("../scenarios/m05-interrupt.yaml"),
    ),
    (
        "m06-step-restart",
        include_str!("../scenarios/m06-step-restart.yaml"),
    ),
    ("m07-steer", include_str!("../scenarios/m07-steer.yaml")),
    (
        "m09-session-tree",
        include_str!("../scenarios/m09-session-tree.yaml"),
    ),
    (
        "m10-goal-todo",
        include_str!("../scenarios/m10-goal-todo.yaml"),
    ),
    (
        "m11-flow-run",
        include_str!("../scenarios/m11-flow-run.yaml"),
    ),
    (
        "m12-flow-author",
        include_str!("../scenarios/m12-flow-author.yaml"),
    ),
    ("m13-jobs", include_str!("../scenarios/m13-jobs.yaml")),
    (
        "m14-attention",
        include_str!("../scenarios/m14-attention.yaml"),
    ),
    (
        "m15-reconnect",
        include_str!("../scenarios/m15-reconnect.yaml"),
    ),
    ("m18-errors", include_str!("../scenarios/m18-errors.yaml")),
    (
        "m20-first-run",
        include_str!("../scenarios/m20-first-run.yaml"),
    ),
    ("m16-storm", include_str!("../scenarios/m16-storm.yaml")),
    ("m22-xforge", include_str!("../scenarios/m22-xforge.yaml")),
];

pub fn seed(name: &str) -> Option<&'static str> {
    SEEDS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

pub fn scenario(name: &str) -> Option<&'static str> {
    SCENARIOS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

pub fn seed_names() -> impl Iterator<Item = &'static str> {
    SEEDS.iter().map(|(n, _)| *n).chain(std::iter::once("busy"))
}

pub fn scenario_names() -> impl Iterator<Item = &'static str> {
    SCENARIOS.iter().map(|(n, _)| *n)
}
