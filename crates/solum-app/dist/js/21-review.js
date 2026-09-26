"use strict";
/* ============================== 复盘 ============================== */
$("reviewBtn").addEventListener("click", async () => {
  try {
    const r = await invoke("review", {
      days: parseInt($("reviewDays").value || "7", 10),
      now: nowParam(),
      styled: $("reviewStyled").checked,
    });
    $("reviewOut").style.display = "block";
    $("reviewOut").textContent = r.text;
    const m = $("reviewMode");
    m.style.display = "inline-flex";
    m.textContent = "";
    if (r.styled) m.appendChild(tag("已按人格改写", "ok"));
    else if ($("reviewStyled").checked) m.appendChild(tag("离线原文（云端不可用或校验未过）"));
    else m.appendChild(tag("离线原文"));
  } catch (e) { toast(String(e), "danger"); }
});
