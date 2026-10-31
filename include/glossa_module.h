/*
 * glossa_module.h — the Glossa native module ABI, protocol version 1.
 *
 * A native module is a shared library (.dll / .so / .dylib) that exports a
 * single function, glossa_module_v1, returning a table of two functions.
 * Every request, reply and host call crosses the boundary as a
 * NUL-terminated UTF-8 JSON string, so there are no structs to keep in step
 * beyond the two tables below, no C++ name mangling and no Rust ABI: a module
 * built by any compiler, in any language with a C FFI, works with every
 * Glossa that speaks the same protocol version.
 *
 * This header mirrors crates/glossa-module-sdk/src/abi.rs exactly. The JSON
 * shapes are documented in crates/glossa-module-sdk/src/wire.rs and
 * docs/modules/native-modules.md.
 *
 * ---------------------------------------------------------------------------
 * Messages
 * ---------------------------------------------------------------------------
 *
 *   method       params (JSON object)                      result
 *   translate    {text, from, to, style?, glossary?, options}  {text, detected_from?, ...}
 *   detect       {text, options}                           {lang, confidence?}
 *   dictionary   {text, from, to, exact, options}          {headword, explanations, ...}
 *   speak        {text, lang, slow, options}               {audio_base64, mime}
 *   recognize    {image_base64, lang, ...,  options}       {text, translation?, ...}
 *
 * Languages are BCP 47 codes ("en", "zh-Hans") or "auto". Object keys may
 * arrive in any order and unknown keys must be ignored.
 *
 * Every reply — from the module's call and from the host's call alike — is
 * an envelope:
 *
 *   {"result": <value>}
 *   {"error": {"kind": "<kind>", "message": "<text>", "retry_after_secs": n?}}
 *
 * with <kind> one of: bad_request, unsupported, source_language,
 * target_language, missing_credential, bad_response, http, network,
 * rate_limited, not_found, other. Answer methods you do not implement with
 * kind "unsupported".
 *
 * Host calls (made through GlossaHostVTable while handling a call):
 *
 *   http       {method?, url, headers?, body?, body_base64?, timeout_ms?, binary?}
 *              -> {status, headers, body, body_base64?}
 *              Goes through Glossa's proxy settings and is checked against
 *              the [permissions] network list in module.toml.
 *   log        {level: "trace"|"debug"|"info"|"warn"|"error", message} -> null
 *   cache_get  {key} -> value or null
 *   cache_set  {key, value, ttl_secs} -> null
 *
 * ---------------------------------------------------------------------------
 * Ownership — each side frees only what it allocated
 * ---------------------------------------------------------------------------
 *
 *   - The method and params strings passed to GlossaModuleVTable.call belong
 *     to the host and are valid only until call returns. Copy what you keep.
 *   - The string call returns belongs to the module. The host copies it and
 *     then passes it, exactly once, to GlossaModuleVTable.free. Allocate it
 *     with whatever allocator your free releases (malloc/free is typical).
 *     Returning NULL is allowed but is reported to the user as a broken
 *     module; return an error envelope instead.
 *   - The string GlossaHostVTable.call returns belongs to the host. Pass it,
 *     exactly once, to GlossaHostVTable.free (with the same ctx). Never call
 *     your own free() on it: on Windows the host and module may use
 *     different C runtimes, hence different heaps.
 *   - The method and params strings you pass to GlossaHostVTable.call are
 *     yours; the host does not keep them.
 *
 * ---------------------------------------------------------------------------
 * Threading and lifetime
 * ---------------------------------------------------------------------------
 *
 *   - The host never runs two calls into one module at the same time, but
 *     successive calls may come from different threads. Global state needs
 *     no lock against the host, only against threads you start yourself.
 *   - The GlossaHostVTable pointer and its ctx are valid only during the
 *     call they were passed to, and only on the thread that made that call.
 *     Do not store them; do not call them from another thread (the host
 *     answers such calls with an error).
 *   - The table returned by glossa_module_v1 must stay valid while the
 *     library is loaded; a static const table is the natural choice.
 *   - The library may be unloaded once the host no longer needs it. Do not
 *     leave threads of your own running past your last call.
 *   - There is no time limit the host can enforce: native code cannot be
 *     interrupted. Use timeout_ms on host http calls and keep calls short.
 *
 * ---------------------------------------------------------------------------
 * Safety
 * ---------------------------------------------------------------------------
 *
 *   A native module runs inside the Glossa process with Glossa's privileges.
 *   If it crashes, Glossa crashes. Never let a C++ exception or a Rust panic
 *   escape call or free. Users can switch native modules off entirely.
 */

#ifndef GLOSSA_MODULE_H
#define GLOSSA_MODULE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The protocol version this header describes. A host refuses a module whose
 * table carries a different number. */
#define GLOSSA_PROTOCOL_VERSION 1u

/* The name of the one exported symbol. */
#define GLOSSA_ENTRY_SYMBOL "glossa_module_v1"

/* Marks the entry point as exported from the shared library. */
#if defined(_WIN32) || defined(__CYGWIN__)
#  define GLOSSA_EXPORT __declspec(dllexport)
#elif defined(__GNUC__) || defined(__clang__)
#  define GLOSSA_EXPORT __attribute__((visibility("default")))
#else
#  define GLOSSA_EXPORT
#endif

/* What the host passes into every call: its services, valid for that call
 * only. */
typedef struct GlossaHostVTable {
    /* Opaque host state. Pass it back unchanged as the first argument. */
    void *ctx;

    /* Make a host call. `method` and `params_json` are NUL-terminated UTF-8
     * (params may be NULL, meaning JSON null). Returns a reply envelope the
     * module must release with `free`; never NULL in practice, but check. */
    char *(*call)(void *ctx, const char *method, const char *params_json);

    /* Release a string returned by `call`. NULL is ignored. */
    void (*free)(void *ctx, char *s);
} GlossaHostVTable;

/* What the module exports. */
typedef struct GlossaModuleVTable {
    /* Must be GLOSSA_PROTOCOL_VERSION. Read before anything else. */
    uint32_t protocol_version;

    /* Handle one request. `method` is "translate", "detect", ...;
     * `params_json` is a JSON object; `host` is valid for this call only.
     * Returns a reply envelope allocated by the module. */
    char *(*call)(const char *method, const char *params_json, const GlossaHostVTable *host);

    /* Release a string returned by `call`. Called exactly once per reply. */
    void (*free)(char *s);
} GlossaModuleVTable;

/* The entry point every module exports:
 *
 *   GLOSSA_EXPORT const GlossaModuleVTable *glossa_module_v1(void)
 *   {
 *       static const GlossaModuleVTable table = {
 *           GLOSSA_PROTOCOL_VERSION, my_call, my_free
 *       };
 *       return &table;
 *   }
 */
typedef const GlossaModuleVTable *(*GlossaEntryFn)(void);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* GLOSSA_MODULE_H */
