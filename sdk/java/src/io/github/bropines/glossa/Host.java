package io.github.bropines.glossa;

import java.nio.charset.StandardCharsets;
import java.util.Base64;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * What a handler may ask of Glossa while it handles a request. Every call
 * is a protocol host call: it blocks until Glossa answers and throws
 * {@link ModuleError} when Glossa reports a failure.
 *
 * <p>{@link #fetch} goes through Glossa: the user's proxy and timeouts
 * apply, and only hosts in the manifest's {@code [permissions] network}
 * list are reachable. A Java module is still an ordinary program and may
 * open its own connections; the permissions only govern what Glossa does on
 * its behalf.
 *
 * <p>{@link #call} reaches any host call by name, so a host call added to
 * Glossa after this SDK was written is usable before it gets a typed method
 * here.
 */
public final class Host {
    private final Glossa.Channel channel;
    private final Request request;

    Host(Glossa.Channel channel, Request request) {
        this.channel = channel;
        this.request = request;
    }

    /** Any host call: {@code call("cache_get", Json.obj("key", "token"))}. Returns the parsed result. */
    public Object call(String method, Object params) throws ModuleError {
        return channel.hostCall(method, params == null ? new LinkedHashMap<String, Object>() : params);
    }

    // -- network -----------------------------------------------------------

    /** A GET request. */
    public HttpResponse fetch(String url) throws ModuleError {
        return fetch("GET", url, null, null);
    }

    /** A request with a text body (may be null). Header names are as given. */
    public HttpResponse fetch(String method, String url, Map<String, String> headers, String body) throws ModuleError {
        Map<String, Object> req = new LinkedHashMap<>();
        req.put("method", method.toUpperCase(java.util.Locale.ROOT));
        req.put("url", url);
        req.put("headers", headers == null ? new LinkedHashMap<String, String>() : headers);
        if (body != null) {
            req.put("body", body);
        }
        return http(req);
    }

    /** A POST of a JSON value, with the content type set. */
    public HttpResponse postJson(String url, Object json, Map<String, String> headers) throws ModuleError {
        Map<String, String> h = new LinkedHashMap<>();
        if (headers != null) {
            h.putAll(headers);
        }
        if (h.keySet().stream().noneMatch(k -> k.equalsIgnoreCase("content-type"))) {
            h.put("Content-Type", "application/json; charset=utf-8");
        }
        return fetch("POST", url, h, Json.write(json));
    }

    /**
     * The {@code http} host call with every field of the protocol:
     * {@code method, url, headers, body, body_base64, timeout_ms, binary}.
     */
    public HttpResponse http(Map<String, Object> request) throws ModuleError {
        return new HttpResponse(Json.asMap(call("http", request)));
    }

    // -- the rest ----------------------------------------------------------

    /** Write to Glossa's log: trace, debug, info, warn or error. */
    public void log(String level, String message) throws ModuleError {
        call("log", Json.obj("level", level, "message", message));
    }

    public void debug(String message) throws ModuleError {
        log("debug", message);
    }

    public void info(String message) throws ModuleError {
        log("info", message);
    }

    public void warn(String message) throws ModuleError {
        log("warn", message);
    }

    /**
     * A value this module stored with {@link #cacheSet}, or null. The cache
     * lives in Glossa's memory: it survives this process being stopped for
     * idleness, not a restart of Glossa.
     */
    public Object cacheGet(String key) throws ModuleError {
        return call("cache_get", Json.obj("key", key));
    }

    /** Store a JSON value; {@code ttlSecs} of 0 keeps it until Glossa exits. */
    public void cacheSet(String key, Object value, long ttlSecs) throws ModuleError {
        Map<String, Object> params = Json.obj("key", key, "value", value);
        if (ttlSecs > 0) {
            params.put("ttl_secs", ttlSecs);
        }
        call("cache_set", params);
    }

    /**
     * Where the files from the manifest's {@code [[assets]]} are:
     * {@code {"dir": "...", "files": {"name": "/abs/path"}}}. Glossa starts
     * the module only once they are all downloaded and verified; the same
     * paths are in the environment as {@code GLOSSA_ASSETS_DIR} and
     * {@code GLOSSA_ASSET_<NAME>}.
     */
    @SuppressWarnings("unchecked")
    public Map<String, Object> assets() throws ModuleError {
        Object r = call("assets", null);
        return r instanceof Map ? (Map<String, Object>) r : new LinkedHashMap<>();
    }

    /** One asset's path, or null when the manifest has no asset by that name. */
    public String assetPath(String name) throws ModuleError {
        StringBuilder env = new StringBuilder("GLOSSA_ASSET_");
        for (char c : name.toCharArray()) {
            env.append(Character.isLetterOrDigit(c) && c < 128 ? Character.toUpperCase(c) : '_');
        }
        String fromEnv = System.getenv(env.toString());
        if (fromEnv != null && !fromEnv.isEmpty()) {
            return fromEnv;
        }
        Object files = assets().get("files");
        if (files instanceof Map) {
            Object p = ((Map<?, ?>) files).get(name);
            return p == null ? null : p.toString();
        }
        return null;
    }

    /** The request being handled (for its options). */
    public Request request() {
        return request;
    }

    /** One setting as text, or {@code fallback}. */
    public String option(String key, String fallback) {
        return request.option(key, fallback);
    }

    /**
     * The module's string {@code key} from {@code [strings]} in
     * {@code locales/<lang>.toml}, in Glossa's interface language (English
     * as the fallback), with {@code {0}}, {@code {1}}… replaced by
     * {@code args}. The key itself when there is none. Glossa sends the
     * table with each action, so this makes no host call.
     */
    public String t(String key, Object... args) {
        Object s = Json.asMap(request.get("strings")).get(key);
        String text = s instanceof String ? (String) s : key;
        // One pass, so an argument containing "{1}" is not substituted again.
        java.util.regex.Matcher m = java.util.regex.Pattern.compile("\\{(\\d+)\\}").matcher(text);
        StringBuilder out = new StringBuilder();
        while (m.find()) {
            int i;
            try {
                i = Integer.parseInt(m.group(1));
            } catch (NumberFormatException e) {
                i = -1;
            }
            String with = i >= 0 && i < args.length ? String.valueOf(args[i]) : m.group();
            m.appendReplacement(out, java.util.regex.Matcher.quoteReplacement(with));
        }
        m.appendTail(out);
        return out.toString();
    }

    /** The answer to an {@code http} host call. Any status is an answer; deciding that 4xx fails is yours. */
    public static final class HttpResponse {
        private final Map<String, Object> raw;

        HttpResponse(Map<String, Object> raw) {
            this.raw = raw;
        }

        public int status() {
            Object s = raw.get("status");
            return s instanceof Number ? ((Number) s).intValue() : 0;
        }

        /** Header names are lower-case; repeated headers are joined with a newline. */
        public Map<String, Object> headers() {
            return Json.asMap(raw.get("headers"));
        }

        public String header(String name) {
            Object v = headers().get(name.toLowerCase(java.util.Locale.ROOT));
            return v instanceof String ? (String) v : null;
        }

        /** The body as text (decoded by Glossa from the response's charset). */
        public String body() {
            Object b = raw.get("body");
            if (b instanceof String) {
                return (String) b;
            }
            byte[] bytes = bytes();
            return new String(bytes, StandardCharsets.UTF_8);
        }

        /** The body as bytes: from {@code body_base64} for a binary request, else the text's UTF-8. */
        public byte[] bytes() {
            Object b64 = raw.get("body_base64");
            if (b64 instanceof String) {
                return Base64.getMimeDecoder().decode((String) b64);
            }
            Object b = raw.get("body");
            return b instanceof String ? ((String) b).getBytes(StandardCharsets.UTF_8) : new byte[0];
        }

        public boolean ok() {
            return status() >= 200 && status() < 300;
        }

        /** The body parsed as JSON, requiring a 2xx status ({@code rate_limited} for 429, else {@code http}). */
        public Object json() throws ModuleError {
            if (status() == 429) {
                throw new ModuleError("rate_limited", "HTTP 429: " + clip(body()));
            }
            if (!ok()) {
                throw new ModuleError("http", "HTTP " + status() + ": " + clip(body()));
            }
            try {
                return Json.parse(body());
            } catch (IllegalArgumentException e) {
                throw ModuleError.badResponse("not JSON: " + e.getMessage());
            }
        }

        /** Everything Glossa sent, including fields newer than this SDK. */
        public Map<String, Object> raw() {
            return raw;
        }

        private static String clip(String s) {
            return s.length() > 200 ? s.substring(0, 200) : s;
        }
    }
}
