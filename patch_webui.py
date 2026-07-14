import re
import sys

def patch():
    path = "/Users/horatiubudai/ceo/ShrimpAI/Clawie-full/Clawie/rust-clawie/rust/crates/rusty-claude-cli/src/webui.rs"
    with open(path, "r") as f:
        content = f.read()

    # Add to settings-provider
    content = content.replace(
        '<option value="gemini">Google (Gemini)</option>',
        '<option value="gemini">Google (Gemini)</option>\n                <option value="xai">xAI (Grok)</option>\n                <option value="kimi">Moonshot AI (Kimi)</option>'
    )
    
    # Add to settings-model
    content = content.replace(
        '<option value="gemini-3.5-flash">gemini-3.5-flash</option>',
        '<option value="gemini-3.5-flash">gemini-3.5-flash</option>\n                <option value="grok-3">grok-3</option>\n                <option value="grok-2">grok-2</option>\n                <option value="moonshot-v1-auto">moonshot-v1-auto</option>\n                <option value="moonshot-v1-32k">moonshot-v1-32k</option>\n                <option value="moonshot-v1-128k">moonshot-v1-128k</option>'
    )

    # Add HTML fields
    html_addition = """                <div class="settings-group">
                  <label for="settings-gemini-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom Gemini Base URL (optional)</label>
                  <input id="settings-gemini-url" placeholder="https://generativelanguage.googleapis.com/v1beta/openai" autocomplete="off">
                </div>
              </div>
              <div id="settings-group-xai" style="display: none; padding-top: 10px; border-top: 1px solid var(--border-color); margin-top: 15px;">
                <div class="settings-group">
                  <label for="settings-xai-key" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">xAI API Key (Grok)</label>
                  <div style="position: relative; display: flex; align-items: center;">
                    <input id="settings-xai-key" type="password" placeholder="xai-..." autocomplete="off">
                    <button class="password-toggle-btn" type="button" data-target="settings-xai-key" title="Toggle visibility">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>
                    </button>
                  </div>
                  <button type="button" class="test-connection-btn" data-provider="xai">Test Connection</button>
                  <div class="test-connection-result" style="display: none; margin-top: 5px; font-size: 0.75rem;"></div>
                </div>
                <div class="settings-group">
                  <label for="settings-xai-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom xAI Base URL (optional)</label>
                  <input id="settings-xai-url" placeholder="https://api.x.ai/v1" autocomplete="off">
                </div>
              </div>
              <div id="settings-group-kimi" style="display: none; padding-top: 10px; border-top: 1px solid var(--border-color); margin-top: 15px;">
                <div class="settings-group">
                  <label for="settings-kimi-key" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Moonshot API Key (Kimi)</label>
                  <div style="position: relative; display: flex; align-items: center;">
                    <input id="settings-kimi-key" type="password" placeholder="sk-..." autocomplete="off">
                    <button class="password-toggle-btn" type="button" data-target="settings-kimi-key" title="Toggle visibility">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path><circle cx="12" cy="12" r="3"></circle></svg>
                    </button>
                  </div>
                  <button type="button" class="test-connection-btn" data-provider="kimi">Test Connection</button>
                  <div class="test-connection-result" style="display: none; margin-top: 5px; font-size: 0.75rem;"></div>
                </div>
                <div class="settings-group">
                  <label for="settings-kimi-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom Moonshot Base URL (optional)</label>
                  <input id="settings-kimi-url" placeholder="https://api.moonshot.cn/v1" autocomplete="off">
                </div>
              </div>"""

    content = content.replace(
        """                <div class="settings-group">
                  <label for="settings-gemini-url" style="font-size: 0.65rem; color: var(--text-muted); text-transform: none; letter-spacing: normal;">Custom Gemini Base URL (optional)</label>
                  <input id="settings-gemini-url" placeholder="https://generativelanguage.googleapis.com/v1beta/openai" autocomplete="off">
                </div>
              </div>""",
        html_addition
    )

    # JS localstorage payload for /chat
    content = content.replace(
        "gemini_api_key: localStorage.getItem('clawie-gemini-key') || '',",
        "gemini_api_key: localStorage.getItem('clawie-gemini-key') || '',\n            xai_api_key: localStorage.getItem('clawie-xai-key') || '',\n            kimi_api_key: localStorage.getItem('clawie-kimi-key') || '',"
    )
    content = content.replace(
        "gemini_base_url: localStorage.getItem('clawie-gemini-url') || '',",
        "gemini_base_url: localStorage.getItem('clawie-gemini-url') || '',\n            xai_base_url: localStorage.getItem('clawie-xai-url') || '',\n            kimi_base_url: localStorage.getItem('clawie-kimi-url') || '',"
    )

    # JS DOM queries
    content = content.replace(
        "const settingsGeminiUrl = document.querySelector('#settings-gemini-url');",
        "const settingsGeminiUrl = document.querySelector('#settings-gemini-url');\n    const settingsXaiKey = document.querySelector('#settings-xai-key');\n    const settingsXaiUrl = document.querySelector('#settings-xai-url');\n    const settingsKimiKey = document.querySelector('#settings-kimi-key');\n    const settingsKimiUrl = document.querySelector('#settings-kimi-url');"
    )

    # JS parsing manifest env
    content = content.replace(
        "if (s.GEMINI_BASE_URL) localStorage.setItem('clawie-gemini-url', s.GEMINI_BASE_URL);",
        "if (s.GEMINI_BASE_URL) localStorage.setItem('clawie-gemini-url', s.GEMINI_BASE_URL);\n          if (s.XAI_API_KEY) localStorage.setItem('clawie-xai-key', s.XAI_API_KEY);\n          if (s.XAI_BASE_URL) localStorage.setItem('clawie-xai-url', s.XAI_BASE_URL);\n          if (s.MOONSHOT_API_KEY) localStorage.setItem('clawie-kimi-key', s.MOONSHOT_API_KEY);\n          if (s.MOONSHOT_BASE_URL) localStorage.setItem('clawie-kimi-url', s.MOONSHOT_BASE_URL);"
    )

    # JS setting DOM fields from localstorage
    content = content.replace(
        "settingsGeminiUrl.value = localStorage.getItem('clawie-gemini-url') || '';",
        "settingsGeminiUrl.value = localStorage.getItem('clawie-gemini-url') || '';\n      settingsXaiKey.value = localStorage.getItem('clawie-xai-key') || '';\n      settingsXaiUrl.value = localStorage.getItem('clawie-xai-url') || '';\n      settingsKimiKey.value = localStorage.getItem('clawie-kimi-key') || '';\n      settingsKimiUrl.value = localStorage.getItem('clawie-kimi-url') || '';"
    )

    # JS UI panel toggling
    content = content.replace(
        "document.getElementById('settings-group-gemini').style.display = 'none';",
        "document.getElementById('settings-group-gemini').style.display = 'none';\n      document.getElementById('settings-group-xai').style.display = 'none';\n      document.getElementById('settings-group-kimi').style.display = 'none';"
    )
    content = content.replace(
        "} else if (provider === 'gemini') {\n        document.getElementById('settings-group-gemini').style.display = 'block';\n      }",
        "} else if (provider === 'gemini') {\n        document.getElementById('settings-group-gemini').style.display = 'block';\n      } else if (provider === 'xai') {\n        document.getElementById('settings-group-xai').style.display = 'block';\n      } else if (provider === 'kimi') {\n        document.getElementById('settings-group-kimi').style.display = 'block';\n      }"
    )

    # JS test connection saving
    content = content.replace(
        "localStorage.setItem('clawie-gemini-url', settingsGeminiUrl.value.trim());",
        "localStorage.setItem('clawie-gemini-url', settingsGeminiUrl.value.trim());\n      localStorage.setItem('clawie-xai-key', settingsXaiKey.value.trim());\n      localStorage.setItem('clawie-xai-url', settingsXaiUrl.value.trim());\n      localStorage.setItem('clawie-kimi-key', settingsKimiKey.value.trim());\n      localStorage.setItem('clawie-kimi-url', settingsKimiUrl.value.trim());"
    )
    content = content.replace(
        "payload.base_url = settingsGeminiUrl.value.trim() || undefined;\n        }",
        "payload.base_url = settingsGeminiUrl.value.trim() || undefined;\n        } else if (provider === 'xai') {\n          payload.api_key = settingsXaiKey.value.trim();\n          payload.base_url = settingsXaiUrl.value.trim() || undefined;\n        } else if (provider === 'kimi') {\n          payload.api_key = settingsKimiKey.value.trim();\n          payload.base_url = settingsKimiUrl.value.trim() || undefined;\n        }"
    )

    with open(path, "w") as f:
        f.write(content)
    print("Patched webui.rs")

patch()
