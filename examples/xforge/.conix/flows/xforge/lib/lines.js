// 三条线（docs/design/xforge.md §1）：治理文件的 lines 给每条线一个模型，review 的检视点决定检视线在哪几处干活。
// 门禁 lines 与自检用同一份判法：自检说没问题的，运行起来这一站不会挡。

export const LINES = ["build", "tests", "review"];
export const STAGES = ["spec", "design", "tests-design", "code", "tests", "verify"];
// 有检视点的路径：quick 只有开发线。
export const REVIEWED = ["solid", "major"];

// path 这条路径开着的检视点。
export function stagesOn(settings, path) {
  const m = ((settings || {}).review || {})[path];
  if (!m || typeof m !== "object") return [];
  return STAGES.filter((s) => m[s] === true);
}

// 一条线的模型：写了就是它，没写是 null（实例的缺省模型）。
export function modelOf(settings, line) {
  const l = ((settings || {}).lines || {})[line];
  const m = l && typeof l === "object" ? l.model : null;
  return typeof m === "string" && m.trim() ? m.trim() : null;
}

// path 这条路径的检视线不成立的地方：null 就是没问题。开着检视点时检视线要写明模型、与开发线不同。
export function reviewLineProblem(settings, path) {
  const on = stagesOn(settings, path);
  if (!on.length) return null;
  const build = modelOf(settings, "build");
  const review = modelOf(settings, "review");
  const where = `${path} 开着检视点 ${on.join("、")}`;
  if (!review) {
    return `${where}，检视线没写模型（lines.review.model）：会用实例的缺省模型${build ? "" : "，开发线也没写，两条线就是同一个模型"}`;
  }
  if (review === (build || null)) return `${where}，检视线与开发线是同一个模型 ${review}：检视要换一个模型`;
  return null;
}
