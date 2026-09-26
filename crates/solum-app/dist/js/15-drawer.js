"use strict";
const DRAWER_PANES = { documents: "drawerTabDocuments", widgets: "drawerTabWidgets" };
let drawerOpener = null;
function drawerShowPane(pane) {
  for (const [p, tabId] of Object.entries(DRAWER_PANES)) {
    $("pane-" + p).classList.toggle("active", p === pane);
    $(tabId).classList.toggle("active", p === pane);
    $(tabId).setAttribute("aria-selected", String(p === pane));
  }
  if (pane === "documents") refreshDocuments(); else refreshWidgets();
}
function openDrawer(pane) {
  if (!accountLoggedIn) {
    requireAccount(pane === "documents" ? "登录后使用本地资料工作台" : "登录后创建和管理个人组件");
    return false;
  }
  if (!$("drawer").classList.contains("open")) {
    drawerOpener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    $("drawerMask").classList.add("open");
    $("drawer").classList.add("open");
    syncOverlayHistory();
    $("drawer").focus({ preventScroll: true });
  }
  drawerShowPane(pane);
  return true;
}
function closeDrawer() {
  if (!$("drawer").classList.contains("open")) return;
  $("drawerMask").classList.remove("open");
  $("drawer").classList.remove("open");
  const opener = drawerOpener;
  drawerOpener = null;
  syncOverlayHistory();
  if (opener && document.contains(opener)) requestAnimationFrame(() => opener.focus());
}
$("drawerClose").appendChild(icon("x", 17));
$("drawerClose").addEventListener("click", closeDrawer);
$("drawerMask").addEventListener("click", closeDrawer);
$("drawerTabDocuments").addEventListener("click", () => drawerShowPane("documents"));
$("drawerTabWidgets").addEventListener("click", () => drawerShowPane("widgets"));

function toggleComposerMenu(force) {
  const menu = $("composerMenu");
  const show = force != null ? force : menu.hidden;
  if (show === !menu.hidden) return; // 状态没变就别动 history
  if (show) toggleSessionMenu(false);
  menu.hidden = !show;
  $("composerPlus").setAttribute("aria-expanded", String(show));
  syncOverlayHistory();
}
$("composerPlus").appendChild(icon("plus", 18));
{
  const menuItem = (ic, label, hint, onclick) => el("button", { type: "button", onclick: () => { toggleComposerMenu(false); onclick(); } }, [
    icon(ic, 18),
    el("span", null, [el("span", { text: label }), el("span", { class: "mhint", text: hint })]),
  ]);
  $("composerMenu").append(
    menuItem("plus", "上传资料", "PDF / 文档，仅保存在本机", () => accountLoggedIn ? $("documentInput").click() : requireAccount("登录后使用本地资料工作台")),
    menuItem("folder", "资料库", "已上传的本地资料", () => openDrawer("documents")),
    menuItem("doc", "组件库", "自定义组件与制作起点", () => openDrawer("widgets")),
  );
}
$("composerPlus").addEventListener("click", (e) => { e.stopPropagation(); toggleComposerMenu(); });
document.addEventListener("click", (e) => {
  if (!$("composerMenu").hidden && !e.target.closest("#composerMenu")) toggleComposerMenu(false);
});
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if ($("modalMask").classList.contains("open")) return; // 模态优先，由模态自己处理
  if (!$("composerMenu").hidden) { toggleComposerMenu(false); return; }
  closeDrawer();
});

/* ---------------- 主动问询条 ---------------- */
$("checkinIc").appendChild(icon("clock", 18));
$("checkinClose").appendChild(icon("x", 15));
$("checkinClose").addEventListener("click", () => $("checkinBar").classList.remove("show"));
// r 为 string 或 { question, ui }（F18）：条内渲染快捷答选项，自由输入照常。
function showCheckin(r) {
  const q = typeof r === "string" ? r : r.question;
  $("checkinText").textContent = q;
  const old = $("checkinBar").querySelector(".gc");
  if (old) old.remove();
  if (r && r.ui) $("checkinBar").querySelector(".grow").appendChild(renderEnvelope(r.ui, { skipLeadingText: true }));
  $("checkinBar").classList.add("show");
  showView("chat"); // 跳到对话页方便立刻回答
}
