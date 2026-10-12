// Keeps an open panel fresh. The page is whole without this: every tick the
// fleet instance says so on `events`, and the page it then serves replaces
// what is shown.
(() => {
  const lasts = (seconds) =>
    seconds < 60 ? `${seconds}s` : seconds < 3600 ? `${Math.floor(seconds / 60)}m ${seconds % 60}s`
      : `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;

  // The one thing that moves between ticks: how long until the next.
  const count = () => {
    const next = document.querySelector("time.next[data-next]");
    if (!next) return;
    const left = Number(next.dataset.next) - Math.floor(Date.now() / 1000);
    next.textContent = left > 0 ? `next tick in ${lasts(left)}` : "a tick is running";
    next.closest(".tick").classList.toggle("late", left <= 0);
  };

  const lost = (is) => {
    const said = document.querySelector(".lost");
    if (said) said.hidden = !is;
  };

  const fresh = async () => {
    const answer = await fetch("./", { cache: "no-store" });
    const page = new DOMParser().parseFromString(await answer.text(), "text/html");
    const open = [...document.querySelectorAll("details[open]")].map((d) => d.className);
    document.getElementById("panel").replaceWith(page.getElementById("panel"));
    for (const name of open) document.querySelector(`details.${name}`)?.setAttribute("open", "");
    count();
  };

  const events = new EventSource("events");
  events.addEventListener("tick", () => fresh().catch(() => lost(true)));
  // An EventSource reconnects by itself: a fleet instance started again is found again.
  events.onerror = () => lost(true);
  events.onopen = () => fresh().then(() => lost(false), () => lost(true));

  count();
  setInterval(count, 1000);
})();
