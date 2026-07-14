import re
import sys

def patch():
    path = "/Users/horatiubudai/ceo/ShrimpAI/Clawie-full/Clawie/rust-clawie/rust/crates/rusty-claude-cli/src/webui.rs"
    with open(path, "r") as f:
        content = f.read()

    # Update HTML Modal
    html_old = """      <div class="modal-body" style="padding: 1.25rem; display: flex; flex-direction: column; gap: 0.75rem;">
        <label style="font-size: 0.75rem; color: var(--text-secondary); display: block; margin-bottom: 0.25rem;">Select or type the destination path (relative to workspace root):</label>
        <input type="text" id="save-path-input" value="workflow.json" style="width: 100%; padding: 0.6rem 0.75rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--text-primary); font-size: 0.75rem; outline: none; transition: border-color 0.2s;">
        <div style="font-size: 0.68rem; color: var(--text-muted); line-height: 1.3; margin-top: 0.5rem;">
          Workspace Root: <code id="modal-workspace-path" style="color: var(--text-secondary); word-break: break-all;"></code>
        </div>
      </div>"""
    
    html_new = """      <div class="modal-body" style="padding: 1.25rem; display: flex; flex-direction: column; gap: 0.75rem;">
        <label style="font-size: 0.75rem; color: var(--text-secondary); display: block; margin-bottom: 0.25rem;">Select destination:</label>
        <select id="save-destination-select" style="width: 100%; padding: 0.6rem 0.75rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--text-primary); font-size: 0.75rem; outline: none; margin-bottom: 0.25rem;">
          <option value="workspace">Clawie Workspace</option>
          <option value="downloads">Browser Downloads</option>
        </select>
        
        <div id="save-workspace-options">
          <label style="font-size: 0.75rem; color: var(--text-secondary); display: block; margin-bottom: 0.25rem; margin-top: 0.5rem;">Destination path (relative to workspace root):</label>
          <input type="text" id="save-path-input" value="workflow.json" style="width: 100%; padding: 0.6rem 0.75rem; background: var(--bg-main); border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--text-primary); font-size: 0.75rem; outline: none; transition: border-color 0.2s;">
          <div style="font-size: 0.68rem; color: var(--text-muted); line-height: 1.3; margin-top: 0.5rem;">
            Workspace Root: <code id="modal-workspace-path" style="color: var(--text-secondary); word-break: break-all;"></code>
          </div>
        </div>
      </div>"""
    
    content = content.replace(html_old, html_new)

    # Update JS logic
    js_old = """    // Modal Save Confirm Handler
    document.querySelector('#save-path-confirm').addEventListener('click', async () => {
      const filename = savePathInput.value.trim() || 'workflow.json';
      const workflowData = getWorkflowJson();"""
    
    js_new = """    // Modal Save Confirm Handler
    document.querySelector('#save-destination-select').addEventListener('change', (e) => {
      document.querySelector('#save-workspace-options').style.display = e.target.value === 'workspace' ? 'block' : 'none';
    });

    document.querySelector('#save-path-confirm').addEventListener('click', async () => {
      const dest = document.querySelector('#save-destination-select').value;
      const workflowData = getWorkflowJson();

      if (dest === 'downloads') {
        const blob = new Blob([workflowData], {type: 'application/json'});
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = 'workflow.json';
        a.click();
        URL.revokeObjectURL(url);
        
        setStatus('Workflow successfully downloaded!', 'saved');
        if (window.logAutomationEvent) window.logAutomationEvent('Workflow successfully saved as JSON file to Downloads');
        closeSaveModal();
        return;
      }
      
      const filename = savePathInput.value.trim() || 'workflow.json';"""

    content = content.replace(js_old, js_new)

    with open(path, "w") as f:
        f.write(content)
    print("Patched save modal")

patch()
