// Toolbar button: toggle the panel on chips.gg, or open Limbo from anywhere else.
const CHIPS = /^https:\/\/(www\.)?chips\.gg\//;

chrome.action.onClicked.addListener(async (tab) => {
  if (!tab.id || !CHIPS.test(tab.url || "")) {
    chrome.tabs.create({ url: "https://chips.gg/play/chipsgg-limbo" });
    return;
  }
  try {
    await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      world: "MAIN",
      func: () => window.__FASTBET__ && window.__FASTBET__.toggle(),
    });
  } catch (err) {
    console.warn("[FastBet] toggle failed", err);
  }
});
