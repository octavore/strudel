browser.runtime.onInstalled.addListener(() => {
  browser.menus.create({
    id: 'copy',
    title: 'Copy Tabs',
    contexts: ['page'],
  });
});

const copyTabs = async () => {
  const tabs = await browser.tabs.query({
    currentWindow: true,
    windowType: 'normal',
  });
  const tabUrls = tabs
    .map((tab) => (tab.title ? `[${tab.url}](${tab.title})` : tab.url))
    .join('\n');
  await navigator.clipboard.writeText(tabUrls);
};

browser.menus.onClicked.addListener(async (info) => {
  switch (info.menuItemId) {
    case 'copy':
      await copyTabs();
      return;
  }
});

browser.browserAction.onClicked.addListener(async () => {
  await copyTabs();
});
