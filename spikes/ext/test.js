chrome.runtime.sendNativeMessage('org.iksa.spike', {ping:'iksa'}, response => {document.getElementById('result').textContent=JSON.stringify({response,error:chrome.runtime.lastError?.message});});
