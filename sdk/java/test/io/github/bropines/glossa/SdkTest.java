package io.github.bropines.glossa;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.List;
import java.util.Map;

/**
 * Tests for the JSON codec and the protocol loop, with no test framework:
 * {@code ./build.sh test} compiles and runs this class, which exits non-zero
 * on the first failure.
 */
public final class SdkTest {
    private static int checks;

    public static void main(String[] args) throws Exception {
        parsesEveryKindOfValue();
        writesAndRoundTrips();
        rejectsMalformedJson();
        servesRequestsAndHostCalls();
        runsAnnotatedActions();
        System.out.println("SdkTest: " + checks + " checks passed");
    }

    static void parsesEveryKindOfValue() {
        Map<String, Object> m = Json.asMap(Json.parse(
                " {\"s\":\"a\\\"b\\\\c\\/\\n\\u00e9\\ud83c\\udf0d\",\"i\":-42,\"big\":12345678901234567890,"
                        + "\"f\":1.5e2,\"t\":true,\"n\":null,\"a\":[1,[],{}],\"u\":\"мир\"} "));
        eq(m.get("s"), "a\"b\\c/\né🌍");
        eq(m.get("i"), -42L);
        eq(m.get("big"), 1.2345678901234567e19);
        eq(m.get("f"), 150.0);
        eq(m.get("t"), Boolean.TRUE);
        eq(m.containsKey("n") && m.get("n") == null, true);
        eq(Json.asList(m.get("a")).size(), 3);
        eq(m.get("u"), "мир");
        eq(String.join(",", m.keySet()), "s,i,big,f,t,n,a,u");
        eq(Json.parse("0"), 0L);
        eq(Json.parse("\"\""), "");
    }

    record Result(String text, String detected_from, List<String> alternatives) {}

    enum Kind { NOUN }

    static void writesAndRoundTrips() {
        eq(Json.write(Json.obj("a", 1, "b", "x\"y\n\u0001", "c", null, "d", Arrays.asList(true, 2.5, 3.0))),
                "{\"a\":1,\"b\":\"x\\\"y\\n\\u0001\",\"c\":null,\"d\":[true,2.5,3]}");
        eq(Json.write(new Result("hi", "en", List.of("hey"))),
                "{\"text\":\"hi\",\"detected_from\":\"en\",\"alternatives\":[\"hey\"]}");
        eq(Json.write(new byte[] {1, 2, 3}), "\"AQID\"");
        eq(Json.write(new int[] {1, 2}), "[1,2]");
        eq(Json.write(Kind.NOUN), "\"NOUN\"");
        eq(Json.write(Double.NaN), "null");
        eq(Json.write("мир 🌍 \u2028"), "\"мир 🌍 \\u2028\"");
        String doc = "{\"x\":[1,2,{\"y\":\"z\"}],\"e\":\"\\t\",\"k\":-0.25}";
        eq(Json.write(Json.parse(doc)), doc);
    }

    static void rejectsMalformedJson() {
        for (String bad : new String[] {"", "{", "[1,]", "{\"a\" 1}", "01", "1.", "\"abc", "tru", "{\"a\":1}x",
                "\"\\x\"", "\"a\nb\"", "-", "[1 2]"}) {
            try {
                Json.parse(bad);
                fail("parsed bad JSON: " + bad);
            } catch (Json.ParseException expected) {
                checks++;
            }
        }
        try {
            Json.write(new Object());
            fail("wrote an Object");
        } catch (IllegalArgumentException expected) {
            checks++;
        }
    }

    static void servesRequestsAndHostCalls() throws Exception {
        GlossaModule module = new GlossaModule() {
            @Override
            public Object translate(Request req, Host host) throws Exception {
                Object cached = host.cacheGet("k");
                if ("key".equals(req.text())) {
                    req.requireOption("api_key");
                }
                if ("boom".equals(req.text())) {
                    throw new IllegalStateException("kaput");
                }
                return req.text().toUpperCase() + ":" + cached;
            }

            @Override
            public Object detect(Request req, Host host) {
                return "fr";
            }

            @Override
            public Object handle(Request req, Host host) throws Exception {
                return Json.obj("echo", host.call("image_resize", Json.obj("w", 2)));
            }
        };
        String input = String.join("\n",
                "{\"id\":1,\"method\":\"translate\",\"params\":{\"text\":\"héllo\",\"options\":{}}}",
                "{\"id\":\"h1\",\"result\":42}",
                "not json at all",
                "{\"id\":2,\"method\":\"speak\",\"params\":{\"text\":\"x\"}}",
                "{\"id\":3,\"method\":\"translate\",\"params\":{\"text\":\"key\",\"options\":{\"api_key\":\"\"}}}",
                "{\"id\":\"h2\",\"result\":null}",
                "{\"id\":4,\"method\":\"translate\",\"params\":{\"text\":\"boom\"}}",
                "{\"id\":\"h3\",\"error\":{\"kind\":\"rate_limited\",\"message\":\"slow\",\"retry_after_secs\":5}}",
                "{\"id\":5,\"method\":\"detect\",\"params\":{\"text\":\"bonjour\"}}",
                "{\"id\":6,\"method\":\"future_method\",\"params\":{}}",
                "{\"id\":\"h4\",\"result\":{\"ok\":true}}",
                "");
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        Glossa.serve(module, new ByteArrayInputStream(input.getBytes(StandardCharsets.UTF_8)), out);
        String[] lines = out.toString(StandardCharsets.UTF_8).split("\n");
        int i = 0;
        eq(lines[i++], "{\"id\":\"h1\",\"method\":\"cache_get\",\"params\":{\"key\":\"k\"}}");
        eq(lines[i++], "{\"id\":1,\"result\":{\"text\":\"HÉLLO:42\"}}");
        eq(lines[i++], "{\"id\":2,\"error\":{\"kind\":\"unsupported\",\"message\":\"speak is not supported by this module\"}}");
        eq(lines[i++], "{\"id\":\"h2\",\"method\":\"cache_get\",\"params\":{\"key\":\"k\"}}");
        eq(lines[i++], "{\"id\":3,\"error\":{\"kind\":\"missing_credential\",\"message\":\"api_key\"}}");
        eq(lines[i++], "{\"id\":\"h3\",\"method\":\"cache_get\",\"params\":{\"key\":\"k\"}}");
        // The host call failed, so the handler's ModuleError carries Glossa's kind.
        eq(lines[i++], "{\"id\":4,\"error\":{\"kind\":\"rate_limited\",\"message\":\"slow\",\"retry_after_secs\":5}}");
        eq(lines[i++], "{\"id\":5,\"result\":{\"lang\":\"fr\",\"confidence\":1}}");
        eq(lines[i++], "{\"id\":\"h4\",\"method\":\"image_resize\",\"params\":{\"w\":2}}");
        eq(lines[i++], "{\"id\":6,\"result\":{\"echo\":{\"ok\":true}}}");
        eq(lines.length, i);

        // An exception that is not a ModuleError is "other", with its stack trace.
        String boom = "{\"id\":1,\"method\":\"translate\",\"params\":{\"text\":\"boom\"}}\n{\"id\":\"h1\",\"result\":null}\n";
        out.reset();
        Glossa.serve(module, new ByteArrayInputStream(boom.getBytes(StandardCharsets.UTF_8)), out);
        Map<String, Object> reply = Json.asMap(Json.parse(out.toString(StandardCharsets.UTF_8).split("\n")[1]));
        Map<String, Object> err = Json.asMap(reply.get("error"));
        eq(err.get("kind"), "other");
        String message = (String) err.get("message");
        eq(message.startsWith("java.lang.IllegalStateException: kaput") && message.contains("at io.github"), true);
    }

    /** A module with actions, as an author writes one: a public class. */
    public static final class ActionModule implements GlossaModule {
        @Action("list_models")
        public Object listModels(Request ev, Host host) {
            return Json.obj("choices", Json.obj("model", List.of("small", Json.obj("value", "large", "label", "L"))));
        }

        @Action("checkKey")
        public Object check(Request ev, Host host) throws ModuleError {
            if (!"good".equals(ev.option("api_key", ""))) {
                return Json.obj("errors", Json.obj("api_key", host.t("bad_key")));
            }
            return host.t("plan", "pro") + " " + ev.action() + " " + ev.uiLang();
        }

        @Action("quiet")
        public Object quiet(Request ev, Host host) {
            return null;
        }
    }

    static void runsAnnotatedActions() throws Exception {
        String strings = "\"strings\":{\"plan\":\"Тариф: {0}\",\"bad_key\":\"Не тот ключ\"},\"ui_lang\":\"ru\"";
        String input = String.join("\n",
                "{\"id\":1,\"method\":\"action\",\"params\":{\"action\":\"list_models\",\"handler\":\"list_models\"}}",
                "{\"id\":2,\"method\":\"action\",\"params\":{\"action\":\"test\",\"handler\":\"checkKey\","
                        + "\"options\":{\"api_key\":\"good\"}," + strings + "}}",
                "{\"id\":3,\"method\":\"action\",\"params\":{\"action\":\"test\",\"handler\":\"checkKey\","
                        + "\"options\":{\"api_key\":\"bad\"}," + strings + "}}",
                "{\"id\":4,\"method\":\"action\",\"params\":{\"action\":\"quiet\"}}",
                "{\"id\":5,\"method\":\"action\",\"params\":{\"action\":\"nope\",\"handler\":\"nope\"}}",
                "");
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        Glossa.serve(new ActionModule(), new ByteArrayInputStream(input.getBytes(StandardCharsets.UTF_8)), out);
        String[] lines = out.toString(StandardCharsets.UTF_8).split("\n");
        eq(lines[0], "{\"id\":1,\"result\":{\"choices\":{\"model\":[\"small\",{\"value\":\"large\",\"label\":\"L\"}]}}}");
        eq(lines[1], "{\"id\":2,\"result\":{\"notice\":\"Тариф: pro test ru\"}}");
        eq(lines[2], "{\"id\":3,\"result\":{\"errors\":{\"api_key\":\"Не тот ключ\"}}}");
        eq(lines[3], "{\"id\":4,\"result\":{}}");
        eq(Json.string(Json.asMap(Json.parse(lines[4])).get("error"), "kind", ""), "unsupported");
        eq(lines.length, 5);
    }

    static void eq(Object actual, Object expected) {
        checks++;
        if (actual == null ? expected != null : !actual.equals(expected)) {
            fail("expected <" + expected + "> but got <" + actual + ">");
        }
    }

    static void fail(String message) {
        AssertionError e = new AssertionError(message);
        e.printStackTrace();
        System.exit(1);
    }
}
