// 三条线（docs/design/xforge.md §1）：治理文件的 lines 给每条线一个模型，review 的检视点决定检视线在哪几处干活。
// 门禁 lines 与自检用同一份判法：自检说的告警，运行起来这一站也会说。

export const LINES = ["build", "tests", "review"];
export const STAGES = ["spec", "design", "tests-design", "code", "tests", "verify"];
// 每条路径有哪几个检视点：quick 只有开发线；solid 只检视设计环节（两条线的设计）；major 整条链都能检视。
export const STAGES_OF = { solid: ["design", "tests-design"], major: STAGES };
export const REVIEWED = Object.keys(STAGES_OF);

// path 这条路径开着的检视点。
export function stagesOn(settings, path) {
  const m = ((settings || {}).review || {})[path];
  if (!m || typeof m !== "object") return [];
  return (STAGES_OF[path] || []).filter((s) => m[s] === true);
}

// 一条线的模型：写了就是它，没写是 null（实例的缺省模型）。
export function modelOf(settings, line) {
  const l = ((settings || {}).lines || {})[line];
  const m = l && typeof l === "object" ? l.model : null;
  return typeof m === "string" && m.trim() ? m.trim() : null;
}

// path 这条路径的检视线该让人知道的地方：null 就是没有。开着检视点时，检视线没写模型、或与开发线同一个模型，
// 检视就不是另一双眼睛——照样能跑（一个人只配了一家模型也要能用），但人要知道。
export function reviewLineWarning(settings, path) {
  const on = stagesOn(settings, path);
  if (!on.length) return null;
  const build = modelOf(settings, "build");
  const review = modelOf(settings, "review");
  const where = `${path} 开着检视点 ${on.join("、")}`;
  if (!review) {
    return `${where}，检视线没写模型（lines.review.model）：用实例的缺省模型${build ? "，可能与开发线相同" : "，开发线也没写，两条线就是同一个模型"}`;
  }
  if (review === (build || null)) return `${where}，检视线与开发线是同一个模型 ${review}：检视不是另一双眼睛`;
  return null;
}
