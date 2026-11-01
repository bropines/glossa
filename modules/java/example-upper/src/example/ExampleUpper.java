package example;

import io.github.bropines.glossa.Glossa;
import io.github.bropines.glossa.GlossaModule;
import io.github.bropines.glossa.Host;
import io.github.bropines.glossa.Json;
import io.github.bropines.glossa.Request;
import java.util.Locale;

/** The smallest useful Java module: it uppercases text. */
public final class ExampleUpper implements GlossaModule {
    /** How many requests this process has served; the process stops when idle, so it restarts at 0. */
    private int served;

    @Override
    public Object translate(Request req, Host host) throws Exception {
        served++;
        host.debug("uppercasing " + req.text().length() + " characters (request " + served + ")");
        // req.from() / req.to() are BCP 47 codes ("auto" when unknown);
        // req.options() holds this module's settings.
        return Json.obj("text", req.text().toUpperCase(Locale.ROOT), "detected_from", "en");
    }

    @Override
    public Object recognize(Request req, Host host) {
        // A real engine would read req.image() (PNG, JPEG or WebP bytes).
        return Json.obj(
                "text", "Recognised by Java (" + req.image().length + " bytes)",
                "detected_lang", "en");
    }

    public static void main(String[] args) {
        Glossa.run(new ExampleUpper());
    }
}
