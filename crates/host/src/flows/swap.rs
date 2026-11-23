//! 能整体换掉的流程包：插件启停时按启用的插件重建 `DirPacks`、换进来。
//!
//! 派工器的 `CatalogPacks` 只收一次内层（`set_inner` 第二次什么都不改），所以在它下面垫这一层。
//! 已经启动的运行不受影响：定义、策略、门禁启动时就钉在运行记录里（harnix-flow `DirPacks::with_plugin`）。

use std::sync::{Arc, RwLock};

use harnix_flow::Packs;
use harnix_flow::{Catalog, FlowDef, Origin, PartKind, Withheld};

pub struct SwapPacks(RwLock<Arc<dyn Packs>>);

impl SwapPacks {
    pub fn new(p: Arc<dyn Packs>) -> Self {
        SwapPacks(RwLock::new(p))
    }

    pub fn set(&self, p: Arc<dyn Packs>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = p;
    }

    fn get(&self) -> Arc<dyn Packs> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Packs for SwapPacks {
    fn flows(&self) -> Vec<String> {
        self.get().flows()
    }
    fn flow(&self, name: &str) -> Option<(FlowDef, String)> {
        self.get().flow(name)
    }
    fn catalog(&self) -> Catalog {
        self.get().catalog()
    }
    fn flow_text(&self, name: &str) -> Option<String> {
        self.get().flow_text(name)
    }
    fn part(&self, kind: PartKind, name: &str) -> Option<(String, String)> {
        self.get().part(kind, name)
    }
    fn parts(&self, kind: PartKind) -> Vec<String> {
        self.get().parts(kind)
    }
    fn writable(&self) -> bool {
        self.get().writable()
    }
    fn origin(&self, flow: &str) -> Origin {
        self.get().origin(flow)
    }
    fn part_origin(&self, kind: PartKind, name: &str) -> Origin {
        self.get().part_origin(kind, name)
    }
    fn source(&self, flow: &str) -> Option<String> {
        self.get().source(flow)
    }
    fn part_source(&self, kind: PartKind, name: &str) -> Option<String> {
        self.get().part_source(kind, name)
    }
    fn save_flow(&self, yaml: &str) -> Result<String, String> {
        self.get().save_flow(yaml)
    }
    fn delete_flow(&self, name: &str) -> Result<(), String> {
        self.get().delete_flow(name)
    }
    fn save_part(&self, kind: PartKind, name: &str, text: &str) -> Result<String, String> {
        self.get().save_part(kind, name, text)
    }
    fn delete_part(&self, kind: PartKind, name: &str) -> Result<(), String> {
        self.get().delete_part(kind, name)
    }
    fn project(&self, root: &str) -> Option<Arc<dyn Packs>> {
        self.get().project(root)
    }
    fn withheld_flows(&self) -> Vec<(String, Withheld)> {
        self.get().withheld_flows()
    }
}
