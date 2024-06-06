const UNSUPPORTED_SCHEMES = ['about:', 'data:', 'chrome:', 'file:'];
const DEFAULT_URLS = [
    'https://support.mozilla.org/products/firefox',
    'https://support.mozilla.org/kb/customize-firefox-controls-buttons-and-toolbars?utm_source=firefox-browser&utm_medium=default-bookmarks&utm_campaign=customize',
    'https://www.mozilla.org/contribute/',
    'https://www.mozilla.org/about/',
    'https://www.mozilla.org/firefox/?utm_medium=firefox-desktop&utm_source=bookmarks-toolbar&utm_campaign=new-users&utm_content=-global'
];
const INTERVAL = 60 * 1000; // 60s * 1000ms => 1m

function isUnsupportedUrl(url) {
    return UNSUPPORTED_SCHEMES.some(scheme => url.startsWith(scheme));
}

function isDefaultUrl(url) {
    return DEFAULT_URLS.includes(url);
}

async function getFolderName(folderId) {
    if (folderId === 'unfiled_____') {
        return null;
    }

    const folderNode = await browser.bookmarks.get(folderId);
    if (folderNode && folderNode.length > 0 && folderNode[0].title) {
        return folderNode[0].title;
    }

    return null;
}

async function processBookmark(bookmark) {
    console.log('Intercepted bookmark:', bookmark);

    if (isUnsupportedUrl(bookmark.url) || isDefaultUrl(bookmark.url)) {
        console.log('Skipping unsupported or default bookmark:', bookmark.url);
        return;
    }

    try {
        const folderName = await getFolderName(bookmark.parentId);

        const response = await fetch('http://localhost:5000/process_bookmark', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({
                title: bookmark.title,
                url: bookmark.url,
                folder: folderName,
                date: new Date(bookmark.dateAdded).toISOString(),
            })
        });

        if (response.ok) {
            console.log('Bookmark processed successfully');
            await browser.bookmarks.remove(bookmark.id);
        } else {
            console.error('Failed to process bookmark');
            alert('Failed to process bookmark. Please try again later.');
        }
    } catch (error) {
        console.error('Error sending bookmark:', error);
        alert('Failed to process bookmark. Please try again later.');
    }
}

async function isBackendAvailable() {
    try {
        const response = await fetch('http://localhost:5000/health');
        return response.ok;
    } catch (error) {
        return false;
    }
}

async function processAllBookmarks() {
    try {
        const bookmarks = await browser.bookmarks.getTree();
        const queue = [];

        function traverse(bookmarks) {
            for (let bookmark of bookmarks) {
                if (bookmark.url && !isUnsupportedUrl(bookmark.url) && !isDefaultUrl(bookmark.url)) {
                    queue.push(bookmark);
                }
                if (bookmark.children) {
                    traverse(bookmark.children);
                }
            }
        }

        traverse(bookmarks);

        for (let bookmark of queue) {
            if (await isBackendAvailable()) {
                await processBookmark(bookmark);
            } else {
                console.log('API is not available. Bookmark processing will be retried later.');
                return;
            }
        }
    } catch (error) {
        console.error('Error processing all bookmarks:', error);
    }
}

async function retryUnprocessedBookmarks() {
    if (await isBackendAvailable()) {
        await processAllBookmarks();
    } else {
        console.log('API is not available. Bookmark processing will be retried later.');
    }
}

browser.runtime.onInstalled.addListener(() => {
    console.log('Extension installed, processing all existing bookmarks.');
    processAllBookmarks();
    setInterval(retryUnprocessedBookmarks, INTERVAL);
});

browser.bookmarks.onCreated.addListener(async (id, bookmark) => {
    if (await isBackendAvailable()) {
        await processBookmark(bookmark);
    } else {
        console.log('API is not available. Bookmark processing will be retried later.');
    }
});
