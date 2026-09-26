"use strict";
/* ============================== 说明问号 ============================== */
// 静态 HTML 里的 .qmark 只留一个空按钮，图标统一在这里补上。
// 说明是浮窗（不推动布局）：点问号开/关，点浮窗外任意处或 Esc 关闭。
document.querySelectorAll(".qmark").forEach((b) => b.appendChild(icon("help", 12)));
function closeDescPopovers(except) {
  document.querySelectorAll(".vhead .desc:not([hidden])").forEach((d) => {
    if (d === except) return;
    d.hidden = true;
    const b = document.querySelector(`.qmark[aria-controls="${d.id}"]`);
    if (b) b.setAttribute("aria-expanded", "false");
  });
}
document.addEventListener("click", (e) => {
  const btn = e.target.closest(".qmark");
  if (!btn) {
    if (!e.target.closest(".vhead .desc")) closeDescPopovers();
    return;
  }
  const target = document.getElementById(btn.getAttribute("aria-controls"));
  if (!target) return;
  target.hidden = !target.hidden;
  btn.setAttribute("aria-expanded", String(!target.hidden));
  if (!target.hidden) closeDescPopovers(target);
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeDescPopovers();
});
