"use strict";
/* ============================== 穿戴 ============================== */
async function refreshWearable() {
  const banner = $("healthBanner");
  try {
    const stt = await invoke("health_status");
    banner.textContent = "";
    if (!stt.available) {
      // 桌面没有 Health Connect 是正常稳态，不值得一条琥珀横幅。
      banner.appendChild(el("div", { class: "statusline", style: "margin-bottom:16px;" }, [
        el("span", { class: "sdot" }),
        el("span", { text: "此设备没有 Health Connect（桌面壳本就没有），数据靠多设备同步从手机带过来。" }),
      ]));
    } else if (!stt.granted) {
      banner.appendChild(el("div", { class: "banner", style: "margin-bottom:18px;" }, [
        icon("watch", 18),
        el("div", { class: "grow", text: "尚未授权读取心率 / 步数 / 睡眠。" }),
        el("button", {
          class: "btn primary sm", text: "去授权",
          onclick: async () => {
            try {
              const ok = await invoke("health_request");
              toast(ok ? "已授权" : "未完成授权", ok ? "ok" : undefined);
              refreshWearable();
            } catch (e) { toast(String(e), "danger"); }
          },
        }),
      ]));
    }
  } catch (_) { banner.textContent = ""; /* 老桌面构建没有该 command 也不影响其余 UI */ }

  const box = $("wearableBox");
  try {
    const rows = await invoke("health_trends");
    box.textContent = "";
    if (!rows.length) {
      box.appendChild(emptyBox("heart", "还没有健康趋势", "手机端授权 Health Connect 后，有数据的指标会在本机生成近 7 天摘要。"));
      return;
    }
    const stats = el("div", { class: "hstats" });
    for (const h of rows) {
      const comparison = h.changePercent === null
        ? `最近 7 个完整自然日有 ${h.currentDays} 天数据，数据不足，暂不比较`
        : `较前 7 天 ${h.changePercent > 0 ? "+" : ""}${h.changePercent}% · ${h.currentDays}/${h.previousDays} 天`;
      stats.appendChild(el("div", { class: "hstat" }, [
        el("div", { class: "v" }, [String(h.current), el("small", { text: h.unit })]),
        el("div", { class: "l", text: `${h.label} · ${comparison}` }),
      ]));
    }
    box.appendChild(stats);
    box.appendChild(el("p", { class: "hint", text: "所有计算均在本机完成，仅表示记录变化，不作健康诊断。" }));
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}
