package io.github.bropines.glossa;

import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * A failure Glossa can explain to the user. Throw it from a handler;
 * any other exception is reported as {@code other} with its stack trace.
 *
 * <p>{@code kind} is one of {@link #KINDS}: {@code missing_credential} with
 * the option key as the message ("set api_key in the settings"),
 * {@code rate_limited} with {@code retryAfterSecs}, {@code not_found} for
 * "no result", and so on. Host calls that fail throw it too, with the kind
 * Glossa reported.
 */
public class ModuleError extends Exception {
    private static final long serialVersionUID = 1L;

    /** The error kinds Glossa understands; anything else becomes {@code other}. */
    public static final List<String> KINDS = Arrays.asList(
            "bad_request", "unsupported", "source_language", "target_language", "missing_credential",
            "bad_response", "http", "network", "rate_limited", "not_found", "other");

    public final String kind;
    public final String message;
    /** Seconds to wait before trying again, for {@code rate_limited}; may be null. */
    public final Long retryAfterSecs;

    public ModuleError(String kind, String message) {
        this(kind, message, null);
    }

    public ModuleError(String kind, String message, Long retryAfterSecs) {
        super(KINDS.contains(kind) ? message : kind + ": " + message);
        boolean known = KINDS.contains(kind);
        this.kind = known ? kind : "other";
        this.message = known ? (message == null ? "" : message) : kind + (message == null ? "" : ": " + message);
        this.retryAfterSecs = retryAfterSecs;
    }

    public static ModuleError unsupported(String method) {
        return new ModuleError("unsupported", method + " is not supported by this module");
    }

    public static ModuleError missingCredential(String optionKey) {
        return new ModuleError("missing_credential", optionKey);
    }

    public static ModuleError badResponse(String message) {
        return new ModuleError("bad_response", message);
    }

    public static ModuleError notFound(String message) {
        return new ModuleError("not_found", message);
    }

    /** The error as the protocol carries it. */
    public Map<String, Object> toWire() {
        Map<String, Object> err = new LinkedHashMap<>();
        err.put("kind", kind);
        err.put("message", message);
        if (retryAfterSecs != null) {
            err.put("retry_after_secs", retryAfterSecs);
        }
        return err;
    }

    /** An error object received from Glossa (a failed host call). */
    static ModuleError fromWire(Object err) {
        if (err instanceof Map) {
            Map<String, Object> m = Json.asMap(err);
            Object retry = m.get("retry_after_secs");
            return new ModuleError(
                    Json.string(m, "kind", "other"),
                    Json.string(m, "message", ""),
                    retry instanceof Number ? ((Number) retry).longValue() : null);
        }
        return new ModuleError("other", String.valueOf(err));
    }
}
