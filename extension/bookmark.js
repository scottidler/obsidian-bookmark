// Listen for bookmark creation
browser.bookmarks.onCreated.addListener((id, bookmark) => {
    // Process the new bookmark
    processBookmark(bookmark);

    // Remove the bookmark from Firefox
    browser.bookmarks.remove(id).catch((error) => {
        console.error(`Error removing bookmark: ${error}`);
    });
});

function processBookmark(bookmark) {
    // Log the intercepted bookmark
    console.log('Intercepted bookmark:', bookmark);

    // Send the bookmark to the first local server
    fetch('http://localhost:5000/process_bookmark', {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json'
        },
        body: JSON.stringify(bookmark)
    }).then(response => {
        if (response.ok) {
            console.log('Bookmark processed successfully');
        } else {
            console.error('Failed to process bookmark');
        }
    }).catch(error => {
        console.error('Error sending bookmark:', error);
    });
}

