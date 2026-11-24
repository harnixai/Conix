//! Conix 的渲染层：`FrameState` 是一帧的全部内容，`render` 把它画出来。
//!
//! 这里不排版文字：每一行在 model 里已经按宽度切好，渲染端只按矩形放置、超出部分截掉。
//! 区域矩形只由 `layout::compute` 算——model 与渲染端调用同一个函数，所以鼠标命中测试与
//! 画面对得上。

pub mod frame;
pub mod layout;
mod render;
mod text;
mod theme;

pub use frame::*;
pub use layout::{LayoutSpec, PaneMode, Rect, Regions, ShowPane, Size};
pub use render::render;
pub use text::{display_width, truncate_to_width};
pub use theme::Theme;
