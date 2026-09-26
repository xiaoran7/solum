"use strict";
/* ============================== Android 提醒可靠性 ============================== */
async function refreshReminderDeliveryHealth() {
  const block = $("reminderReliabilityBlock");
  const warning = $("reminderDeliveryWarning");
  const warningText = $("reminderDeliveryWarningText");
  const status = $("reminderDeliveryStatus");
  const qa = $("reminderProcessDeathQa");
  try {
    const health = await invoke("reminder_delivery_health");
    if (!health.platformSupported) {
      block.hidden = true;
      return;
    }
    block.hidden = false;
    const failures = [];
    if (!health.receiverRegistered) failures.push("当前安装包缺少后台提醒接收器，应用进程退出后提醒不会触达；请安装通过发布核对的新版本");
    if (!health.bootReceiverRegistered) failures.push("当前安装包缺少开机恢复组件，设备重启后提醒不会自动重新挂载");
    if (!health.notificationsEnabled) failures.push("系统通知已关闭，即使提醒到点也无法显示；请在系统设置中允许 Solum 通知");
    warning.hidden = failures.length === 0;
    warningText.textContent = failures.join("；");
    const degraded = !health.exact || failures.length > 0;
    status.textContent = "";
    status.appendChild(el("span", { class: "sdot " + (failures.length ? "danger" : degraded ? "amber" : "ok") }));
    status.appendChild(el("span", { class: "t", text: failures.length
      ? "后台提醒交付未就绪"
      : health.exact
        ? "后台接收、重启恢复与精确提醒均已就绪"
        : "后台接收与重启恢复已就绪；当前使用系统弹性调度，提醒可能延迟" }));
    qa.hidden = !health.debugProcessDeathAvailable;
  } catch (error) {
    qa.hidden = true;
    if (!IS_ANDROID) { block.hidden = true; return; }
    block.hidden = false;
    warning.hidden = false;
    warningText.textContent = "无法确认后台提醒组件是否完整，请重新打开应用；若仍出现，请安装通过发布核对的新版本。";
    status.textContent = "";
    status.appendChild(el("span", { class: "sdot danger" }));
    status.appendChild(el("span", { class: "t", text: safeUserError(error, "后台提醒自检失败") }));
  }
}

function requestDebugProcessDeath() {
  openModal("Debug：模拟后台回收", el("div", null, [
    el("div", { class: "preview", text: "将结束 Solum 当前进程，但不清除数据、不修改设置，也不取消系统已经挂载的提醒。" }),
    el("div", { class: "fine", text: "请先创建一条几分钟后到点的提醒。应用关闭后等待系统通知；不要从系统设置点“强行停止”。" }),
  ]), "结束当前进程", async () => {
    try {
      const scheduled = await invoke("debug_simulate_process_death");
      if (!scheduled) throw new Error("当前构建不允许进程回收模拟");
      toast("当前进程即将结束；请等待已挂载提醒到点", "amber");
    } catch (error) {
      toast(safeUserError(error, "无法启动进程回收模拟"), "danger");
    }
  }, { icon: "alert" });
}
$("reminderProcessDeathBtn").addEventListener("click", requestDebugProcessDeath);
