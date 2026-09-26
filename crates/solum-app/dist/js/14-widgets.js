"use strict";
const WIDGET_QUICK_STARTS = [
  ["收支记录", "记录收入、支出和分类", "帮我制作一个收支记录组件，包含金额、分类、日期和备注"],
  ["任务清单", "跟进状态和截止日期", "帮我制作一个任务清单组件，包含任务、状态、截止日期和备注"],
  ["习惯打卡", "记录每天的完成情况", "帮我制作一个习惯打卡组件，包含习惯名称、日期、是否完成和备注"],
  ["自定义组件", "用自己的字段描述开始", "帮我制作一个组件："],
];
function renderWidgetQuickStarts() {
  const box = $("widgetQuickStarts");
  if (!box) return;
  box.textContent = "";
  for (const [title, hint, prompt] of WIDGET_QUICK_STARTS) {
    box.appendChild(el("button", { class: "workbench-choice", type: "button", onclick: () => {
      closeDrawer();
      showView("chat");
      $("chatInput").value = prompt;
      $("chatInput").focus();
    } }, [el("strong", { text: title }), el("span", { text: hint })]));
  }
}
renderWidgetQuickStarts();

/* ---------------- 工作台抽屉与输入框「+」菜单 ---------------- */
// 原顶层「工作台」组并入对话：资料/组件以右侧抽屉呈现，入口在输入框「+」浮窗。