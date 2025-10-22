import RFB from '@novnc/novnc';

var rfb;

document.getElementById("connect").addEventListener("click", async () => {

    let response = await fetch("/connect", { "method": "POST" });

    if (!response.ok) {
        alert("failed to get connection info");
        return;
    }

    let connection_info = await response.json(); // TODO: any error checking at all

    if (rfb) {
        rfb.disconnect();
        rfb = null;
    }

    rfb = new RFB(document.getElementById("screen"), connection_info.ws_uri);
})
