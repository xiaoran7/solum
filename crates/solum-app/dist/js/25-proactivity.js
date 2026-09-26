"use strict";
/* ============================== 主动度 ============================== */
async function refreshProactivity() {
  const box = $("proactivityBox");
  try {
    const dims = await invoke("proactivity_get");
    box.textContent = "";
    for (const d of dims) {
      const seg = el("div", { class: "seg", role: "group", "aria-label": DIM[d.dimension] || d.dimension });
      for (const l of LEVELS) {
        seg.appendChild(el("button", {
          type: "button", class: l === d.level ? "active" : "", text: LEVEL[l],
          onclick: async () => {
            try {
              await invoke("proactivity_set", { dimension: d.dimension, level: l });
              toast(`已设置 ${DIM[d.dimension]} = ${LEVEL[l]}`, "ok");
              refreshProactivity();
            } catch (e) { toast(String(e), "danger"); refreshProactivity(); }
          },
        }));
      }
      box.appendChild(el("div", { class: "solum-dim" }, [
        el("div", { class: "grow" }, [
          el("div", { class: "name", text: DIM[d.dimension] || d.dimension }),
          el("div", { class: "desc", text: DIMDESC[d.dimension] || "" }),
        ]),
        seg,
      ]));
    }
  } catch (e) { box.textContent = ""; box.appendChild(emptyBox("alert", safeUserError(e, "内容暂时无法读取，请稍后重试。"))); }
}
