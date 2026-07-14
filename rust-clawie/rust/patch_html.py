import re
import sys

def patch():
    path = "/Users/horatiubudai/ceo/ShrimpAI/Clawie-full/Clawie/rust-clawie/rust/crates/rusty-claude-cli/src/webui.rs"
    with open(path, "r") as f:
        content = f.read()

    old_html = """                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-gemini-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom Gemini Base URL (optional)</label>
                  <input id="settings-gemini-url" placeholder="https://generativelanguage.googleapis.com/v1beta/openai" autocomplete="off">
                </div>"""
                
    new_html = """                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-gemini-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom Gemini Base URL (optional)</label>
                  <input id="settings-gemini-url" placeholder="https://generativelanguage.googleapis.com/v1beta/openai" autocomplete="off">
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-xai-key" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">xAI API Key (Grok)</label>
                  <div class="password-input-wrapper">
                    <input id="settings-xai-key" type="password" placeholder="xai-..." autocomplete="off">
                    <button class="password-toggle-btn" type="button" data-target="settings-xai-key" title="Toggle visibility">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>
                    </button>
                  </div>
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-xai-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom xAI Base URL (optional)</label>
                  <input id="settings-xai-url" placeholder="https://api.x.ai/v1" autocomplete="off">
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-kimi-key" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Moonshot API Key (Kimi)</label>
                  <div class="password-input-wrapper">
                    <input id="settings-kimi-key" type="password" placeholder="sk-..." autocomplete="off">
                    <button class="password-toggle-btn" type="button" data-target="settings-kimi-key" title="Toggle visibility">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>
                    </button>
                  </div>
                </div>
                <div style="display: flex; flex-direction: column; gap: 0.25rem;">
                  <label for="settings-kimi-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom Moonshot Base URL (optional)</label>
                  <input id="settings-kimi-url" placeholder="https://api.moonshot.cn/v1" autocomplete="off">
                </div>"""
                
    content = content.replace(old_html, new_html)

    with open(path, "w") as f:
        f.write(content)
    print("Patched HTML successfully")

patch()
