package io.github.bropines.glossa;

/**
 * A Glossa module. Implement the methods your {@code module.toml} promises
 * in {@code capabilities}; the rest answer "unsupported".
 *
 * <p>Each handler gets the request and a {@link Host} for calls back into
 * Glossa, and returns the result: a {@code Map} (see {@link Json#obj}), a
 * record, or for {@code translate}, {@code recognize} and {@code detect} a
 * plain {@code String} (the text, or the language code). Throw
 * {@link ModuleError} for a failure Glossa should explain.
 *
 * <p>Calls arrive one at a time on one thread, so a module's fields need
 * no locking. The process may be stopped when idle and started again, so
 * keep what must outlive it in {@link Host#cacheSet}.
 */
public interface GlossaModule {
    /** {@code {text, from, to, style?, glossary?, options}} → {@code {text, detected_from?, …}} or a String. */
    default Object translate(Request req, Host host) throws Exception {
        throw ModuleError.unsupported("translate");
    }

    /** {@code {text, options}} → {@code {lang, confidence?}} or the code as a String. */
    default Object detect(Request req, Host host) throws Exception {
        throw ModuleError.unsupported("detect");
    }

    /** {@code {text, from, to, exact, options}} → an article: headword, pronunciations, explanations, examples… */
    default Object dictionary(Request req, Host host) throws Exception {
        throw ModuleError.unsupported("dictionary");
    }

    /** {@code {text, lang, slow, options}} → {@code {audio_base64, mime}} (a {@code byte[]} is written as base64). */
    default Object speak(Request req, Host host) throws Exception {
        throw ModuleError.unsupported("speak");
    }

    /** {@code {image_base64, lang, translate_to?, preserve_line_breaks, options}} → {@code {text, translation?, detected_lang?}}. */
    default Object recognize(Request req, Host host) throws Exception {
        throw ModuleError.unsupported("recognize");
    }

    /** Any method this interface does not name yet. */
    default Object handle(Request req, Host host) throws Exception {
        throw ModuleError.unsupported(req.method());
    }
}
