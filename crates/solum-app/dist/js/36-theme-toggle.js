"use strict";
function setupThemeToggle() {
  const toggleBtn = $("themeToggleBtn");
  const iconSpan = $("themeToggleIcon");
  const labelSpan = $("themeToggleLabel");
  if (!toggleBtn) return;

  const currentTheme = localStorage.getItem("solum.theme") || "system";

  function applyTheme(theme) {
    if (theme === "dark") {
      document.documentElement.setAttribute("data-theme", "dark");
      document.documentElement.style.colorScheme = "dark";
      if (iconSpan) iconSpan.replaceChildren(icon("clock", 16));
      if (labelSpan) labelSpan.textContent = typeof t === "function" ? t("theme.dark", null, "深色模式") : "深色模式";
    } else if (theme === "light") {
      document.documentElement.setAttribute("data-theme", "light");
      document.documentElement.style.colorScheme = "light";
      if (iconSpan) iconSpan.replaceChildren(icon("bulb", 16));
      if (labelSpan) labelSpan.textContent = typeof t === "function" ? t("theme.light", null, "浅色模式") : "浅色模式";
    } else {
      document.documentElement.removeAttribute("data-theme");
      document.documentElement.style.colorScheme = "light dark";
      if (iconSpan) iconSpan.replaceChildren(icon("refresh", 16));
      if (labelSpan) labelSpan.textContent = typeof t === "function" ? t("theme.system", null, "跟随系统") : "跟随系统";
    }
    localStorage.setItem("solum.theme", theme);
  }

  applyTheme(currentTheme);

  toggleBtn.addEventListener("click", () => {
    const cur = localStorage.getItem("solum.theme") || "system";
    const next = cur === "system" ? "dark" : cur === "dark" ? "light" : "system";
    applyTheme(next);
    const themeName = next === "dark"
      ? (typeof t === "function" ? t("theme.dark") : "深色")
      : next === "light"
      ? (typeof t === "function" ? t("theme.light") : "浅色")
      : (typeof t === "function" ? t("theme.system") : "跟随系统");
    const msg = typeof t === "function" ? t("theme.switched", { theme: themeName }) : "外观已切换为：" + themeName;
    toast(msg, "ok");
  });
}
