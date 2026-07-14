import re
import sys

def patch():
    path = "/Users/horatiubudai/ceo/ShrimpAI/Clawie-full/Clawie/rust-clawie/rust/crates/rusty-claude-cli/src/main.rs"
    with open(path, "r") as f:
        content = f.read()

    # Re-apply Gemini and Kimi fixes
    content = content.replace(
        """        Some(ProviderKind::OpenAi) => default_model_for_provider(ProviderKind::OpenAi),
        None => DEFAULT_MODEL,""",
        """        Some(ProviderKind::OpenAi) => default_model_for_provider(ProviderKind::OpenAi),
        Some(ProviderKind::Gemini) => "gemini-1.5-pro",
        Some(ProviderKind::Kimi) => "moonshot-v1-auto",
        None => DEFAULT_MODEL,"""
    )
    content = content.replace(
        """        ProviderKind::OpenAi => "OpenAI",
        ProviderKind::Xai => "xAI",
    }""",
        """        ProviderKind::OpenAi => "OpenAI",
        ProviderKind::Xai => "xAI",
        ProviderKind::Gemini => "Gemini",
        ProviderKind::Kimi => "Kimi",
    }"""
    )

    with open(path, "w") as f:
        f.write(content)
    print("Patched main.rs")

patch()
