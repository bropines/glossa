/*
 * shout.c — a Glossa native module in plain C: "translates" by uppercasing.
 *
 * It exists to prove the ABI is language-agnostic: no Rust, no SDK, just
 * include/glossa_module.h and the C library. Build it beside module.toml:
 *
 *   Linux:   cc -shared -fPIC -std=c99 -I ../../../include -o libshout.so shout.c
 *   macOS:   cc -shared -std=c99 -I ../../../include -o libshout.dylib shout.c
 *   Windows: x86_64-w64-mingw32-gcc -shared -std=c99 -I ../../../include -o shout.dll shout.c
 *            (or MSVC: cl /LD /I ..\..\..\include shout.c /Fe:shout.dll)
 *
 * A real module would use a JSON library; the tiny scanner below only reads
 * one top-level string member, which is all this example needs.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "glossa_module.h"

/* -- a minimal JSON reader ------------------------------------------------ */

static const char *skip_ws(const char *p)
{
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r')
        p++;
    return p;
}

/* `p` is at an opening quote; return the byte after the closing one. */
static const char *skip_string(const char *p)
{
    for (p++; *p && *p != '"'; p++) {
        if (*p == '\\' && !*++p)
            return NULL;
    }
    return *p == '"' ? p + 1 : NULL;
}

/* Skip one value of any type. */
static const char *skip_value(const char *p)
{
    p = skip_ws(p);
    if (*p == '"')
        return skip_string(p);
    if (*p == '{' || *p == '[') {
        int depth = 0;
        while (*p) {
            if (*p == '"') {
                if (!(p = skip_string(p)))
                    return NULL;
                continue;
            }
            if (*p == '{' || *p == '[') {
                depth++;
            } else if (*p == '}' || *p == ']') {
                if (--depth == 0)
                    return p + 1;
            }
            p++;
        }
        return NULL;
    }
    while (*p && *p != ',' && *p != '}' && *p != ']') /* number, true, false, null */
        p++;
    return p;
}

/* Find the top-level string member `key` of the object `json`. On success,
 * point `*start` at the first byte inside its quotes and return the length
 * of its (still escaped) contents; return -1 when it is missing or not a
 * string. Members may come in any order and nested objects are skipped, so
 * "options": {"text": ...} or "style": "text: ..." never match. */
static long find_string_member(const char *json, const char *key, const char **start)
{
    size_t key_len = strlen(key);
    const char *p = skip_ws(json);
    if (*p++ != '{')
        return -1;
    for (;;) {
        p = skip_ws(p);
        if (*p != '"')
            return -1; /* the closing brace: not found */
        const char *name = p + 1;
        const char *after = skip_string(p);
        if (!after)
            return -1;
        int match = (size_t)(after - 1 - name) == key_len && memcmp(name, key, key_len) == 0;
        p = skip_ws(after);
        if (*p++ != ':')
            return -1;
        p = skip_ws(p);
        if (match) {
            const char *end = *p == '"' ? skip_string(p) : NULL;
            if (!end)
                return -1;
            *start = p + 1;
            return (long)(end - 1 - *start);
        }
        if (!(p = skip_value(p)))
            return -1;
        p = skip_ws(p);
        if (*p++ != ',')
            return -1;
    }
}

/* -- replies -------------------------------------------------------------- */

/* Replies are malloc'd here and released by shout_free below — the host
 * never frees them itself. `message` must already be JSON-safe. */
static char *error_reply(const char *kind, const char *message)
{
    const char *fmt = "{\"error\":{\"kind\":\"%s\",\"message\":\"%s\"}}";
    size_t n = strlen(fmt) + strlen(kind) + strlen(message) + 1;
    char *out = malloc(n);
    if (out)
        snprintf(out, n, fmt, kind, message);
    return out;
}

static char *shout(const char *params, const GlossaHostVTable *host)
{
    const char *text;
    long len = find_string_member(params, "text", &text);
    if (len < 0)
        return error_reply("bad_request", "translate needs a string \\\"text\\\"");

    /* A host call. Its reply belongs to the host: hand it back through the
     * host's own free, never through ours. */
    if (host) {
        char *r = host->call(host->ctx, "log", "{\"level\":\"debug\",\"message\":\"shouting\"}");
        if (r)
            host->free(host->ctx, r);
    }

    static const char head[] = "{\"result\":{\"text\":\"";
    static const char tail[] = "\"}}";
    char *out = malloc(sizeof head - 1 + (size_t)len + sizeof tail);
    if (!out)
        return NULL;
    char *w = out;
    memcpy(w, head, sizeof head - 1);
    w += sizeof head - 1;
    /* Copy the still-escaped string, uppercasing ASCII letters but leaving
     * escape sequences (\n, \", \uXXXX) and UTF-8 bytes exactly as they are,
     * so the result is valid JSON without decoding anything. */
    for (long i = 0; i < len; i++) {
        char c = text[i];
        if (c == '\\' && i + 1 < len) {
            int extra = text[i + 1] == 'u' ? 5 : 1;
            *w++ = c;
            for (int k = 0; k < extra && i + 1 < len; k++)
                *w++ = text[++i];
            continue;
        }
        *w++ = (c >= 'a' && c <= 'z') ? (char)(c - 'a' + 'A') : c;
    }
    memcpy(w, tail, sizeof tail); /* includes the terminating NUL */
    return out;
}

/* -- the ABI ---------------------------------------------------------------- */

static char *shout_call(const char *method, const char *params_json, const GlossaHostVTable *host)
{
    if (!method || !params_json)
        return error_reply("bad_request", "null argument");
    if (strcmp(method, "translate") == 0)
        return shout(params_json, host);
    return error_reply("unsupported", "shout only translates");
}

static void shout_free(char *s)
{
    free(s);
}

GLOSSA_EXPORT const GlossaModuleVTable *glossa_module_v1(void)
{
    static const GlossaModuleVTable table = { GLOSSA_PROTOCOL_VERSION, shout_call, shout_free };
    return &table;
}
