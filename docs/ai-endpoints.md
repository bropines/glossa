# AI endpoints

Glossa translates with large language models through the OpenAI
chat-completions protocol, which almost every LLM service and local server
speaks. There is one built-in translator (**AI Translator**, id `openai`), and
you can add as many endpoints as you like; each becomes a translator of its own
in the service strip, the popup and fail-over.

## Adding one

*Options → Services → Translation → AI endpoints*:

1. Choose a preset and, optionally, a name. Press **Add endpoint**.
2. The endpoint is added at once, enabled, and its settings open below.
   Paste the API key (local servers need none).
3. **Load models** asks the endpoint which models it offers and fills a
   picker; or type a model id into *Model*.
4. **Test** translates a short sample with the settings as they are on the
   page, applied or not, and shows the answer and how long it took.
5. **Apply**.

Rename an endpoint in its row (applied with the rest of the page); **Remove**
deletes it and its settings at once.

| preset | base URL | key | default model (an example) |
|---|---|---|---|
| OpenAI | `https://api.openai.com/v1` | [platform.openai.com](https://platform.openai.com/api-keys) | `gpt-4o-mini` |
| OpenRouter | `https://openrouter.ai/api/v1` | [openrouter.ai/keys](https://openrouter.ai/keys) | `openai/gpt-4o-mini` |
| DeepSeek | `https://api.deepseek.com/v1` | [platform.deepseek.com](https://platform.deepseek.com/api_keys) | `deepseek-chat` |
| Google Gemini | `https://generativelanguage.googleapis.com/v1beta/openai` | [aistudio.google.com](https://aistudio.google.com/apikey) | `gemini-2.0-flash` |
| Groq | `https://api.groq.com/openai/v1` | [console.groq.com](https://console.groq.com/keys) | `llama-3.3-70b-versatile` |
| Mistral | `https://api.mistral.ai/v1` | [console.mistral.ai](https://console.mistral.ai/api-keys) | `mistral-small-latest` |
| Ollama (local) | `http://localhost:11434/v1` | none | `qwen2.5:7b` |
| LM Studio (local) | `http://localhost:1234/v1` | none | the loaded model |
| Other | any OpenAI-compatible URL | as needed | — |

Model names change often; the defaults are only a starting point, and *Load
models* shows what is really there.

## Settings

| setting | what it does |
|---|---|
| API base URL | The API root. A full `…/chat/completions` URL works too. |
| API key | Sent as `Authorization: Bearer …`. Required where the preset says so. |
| Model | The model id. |
| Stream the answer | On: the translation appears as the model writes it (server-sent events). Off: one request, one answer. |
| System prompt | The instructions; `{from}` and `{to}` become language names. The text to translate is always sent as the user message, never mixed into the instructions, so text that reads like an instruction is still only translated. |
| Temperature | 0–2; 0–0.3 suits translation. Not sent when a reasoning effort is set. |
| Reasoning effort | Sent as `reasoning_effort` for reasoning models (OpenAI o-series and GPT-5, DeepSeek R1, Gemini 2.5…). "Not sent" for ordinary models. |
| Hide `<think>` blocks | Drops the reasoning that some local models write before the answer. While a model is still thinking, the popup shows nothing rather than its thoughts. |
| Max answer tokens | 0 sends no limit. |
| Token limit field | `max_tokens`, or `max_completion_tokens` for OpenAI's newer models (the OpenAI preset's default). |
| Timeout, seconds | For the whole answer, streamed or not (default 120). |
| Extra headers | One `Name: value` per line. The OpenRouter preset fills in its app attribution headers. |
| Extra request fields | A JSON object merged into the request: `{"top_p": 0.9}`, OpenRouter's `{"provider": {"sort": "price"}}`, a vendor's own switches. Fields given here override Glossa's; `null` removes one (`{"temperature": null}`). |
| Vision model (text recognition) | The model that reads pictures when this endpoint is the text recognition engine. Empty: the model above. See [ocr.md](ocr.md#ai-endpoints-vision-models). |
| Recognition answer | `auto` (lines with boxes when needed, plain text otherwise), `text`, or `lines`. |
| Box coordinates | `auto`, `pixels`, `1000`, `1000yx` (Gemini) or `fraction`: how the model gives a box. |
| Recognition instructions | A prompt of your own for reading pictures; empty uses Glossa's. |

Settings are stored in `config.toml` like any service's:

```toml
[[services.ai_endpoints]]
id = "ai-deepseek"
name = "DeepSeek"
preset = "deepseek"

[services.options.ai-deepseek]
api_key = "sk-…"
model = "deepseek-chat"
reasoning_effort = ""
```

The key is stored in plain text, as every service key is, unless the keys
are kept in the system keychain (*Options → Basics*); keep the data folder
private.

## Style and glossary

AI translators read the **style** and **glossary** that other services ignore:
the style ("informal", "for a game UI") is added to the instructions, and
glossary terms are listed as translations to use exactly.

## Streaming

With streaming on, Glossa asks for `"stream": true` and shows the text so far
in the popup and the main window as events arrive; the final result replaces
it when the answer is complete, and goes to history. `glossa-cli translate`
prints a streaming answer as it is written when its output is a terminal.

A server that ignores `stream` and answers with plain JSON still works.
Errors sent inside the stream (`{"error": …}`) are reported like any other.
If the endpoint fails part-way and fail-over is on, the next service takes
over and its answer replaces the partial one.

## Troubleshooting

- **"needs api_key"**: the preset requires a key and none is set.
- **401 / 403 with the service's message**: the key is wrong or lacks access
  to the model.
- **404**: usually a wrong base URL (a missing `/v1`) or a model id the
  service does not know; *Load models* shows the valid ids.
- **A model rejects `temperature`** (or another field): set a reasoning
  effort, which leaves temperature out, or remove the field with
  `{"temperature": null}` in *Extra request fields*.
- **Timeouts with large local models**: raise *Timeout*; the first request
  after loading a model is the slowest.
- `glossa-cli -v translate -s <endpoint id> -t ru "test"` shows the request's
  fate in the log.
