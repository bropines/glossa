package io.github.bropines.glossa;

import java.util.Base64;
import java.util.Map;

/**
 * The parameters of one request, with getters for the fields the protocol
 * defines (see {@code docs/modules/protocol.md}). {@link #raw()} has
 * everything, including fields newer than this SDK.
 */
public final class Request {
    private final String method;
    private final Map<String, Object> params;

    public Request(String method, Map<String, Object> params) {
        this.method = method;
        this.params = params;
    }

    /** {@code translate}, {@code detect}, {@code dictionary}, {@code speak}, {@code recognize}… */
    public String method() {
        return method;
    }

    /** The parameters as parsed JSON. */
    public Map<String, Object> raw() {
        return params;
    }

    /** The text to translate, detect, look up or speak. */
    public String text() {
        return string("text", "");
    }

    /** The source language, a BCP 47 code; {@code auto} when unknown. */
    public String from() {
        return string("from", "auto");
    }

    /** The target language. */
    public String to() {
        return string("to", "");
    }

    /** {@code speak} and {@code recognize}: the language of the text. */
    public String lang() {
        return string("lang", "auto");
    }

    /** {@code recognize}: the target language when the user asked to translate too, else null. */
    public String translateTo() {
        return string("translate_to", null);
    }

    /** {@code recognize}: the image (PNG, JPEG or WebP), decoded. */
    public byte[] image() {
        String b64 = string("image_base64", null);
        return b64 == null ? new byte[0] : Base64.getMimeDecoder().decode(b64);
    }

    /** The module's settings with defaults filled in. */
    public Map<String, Object> options() {
        return Json.asMap(params.get("options"));
    }

    /** One setting as text, or {@code fallback} when it is unset or empty. */
    public String option(String key, String fallback) {
        Object v = options().get(key);
        if (v == null || "".equals(v)) {
            return fallback;
        }
        return v instanceof String ? (String) v : Json.write(v);
    }

    /** A required setting: its value, or a {@code missing_credential} error naming it. */
    public String requireOption(String key) throws ModuleError {
        String v = option(key, null);
        if (v == null) {
            throw ModuleError.missingCredential(key);
        }
        return v;
    }

    /** {@code action}: the id of the {@code [[actions]]} entry that was pressed. */
    public String action() {
        return string("action", "");
    }

    /** {@code action}: the panel's state; empty on the settings page. */
    public Map<String, Object> state() {
        return Json.asMap(params.get("state"));
    }

    /** {@code action}: Glossa's interface language ({@code en}, {@code ru}…). */
    public String uiLang() {
        return string("ui_lang", "en");
    }

    public String string(String key, String fallback) {
        Object v = params.get(key);
        return v instanceof String ? (String) v : fallback;
    }

    public boolean bool(String key, boolean fallback) {
        Object v = params.get(key);
        return v instanceof Boolean ? (Boolean) v : fallback;
    }

    public Object get(String key) {
        return params.get(key);
    }
}
