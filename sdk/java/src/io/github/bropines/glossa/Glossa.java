package io.github.bropines.glossa;

import java.io.BufferedReader;
import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.nio.charset.StandardCharsets;
import java.util.Map;

/**
 * The Glossa module SDK for Java: runs a {@link GlossaModule} over the module
 * protocol on stdin and stdout.
 *
 * <pre>{@code
 * public final class Main implements GlossaModule {
 *     public Object translate(Request req, Host host) {
 *         return req.text().toUpperCase();
 *     }
 *     public static void main(String[] args) {
 *         Glossa.run(new Main());
 *     }
 * }
 * }</pre>
 *
 * <p>Framing: one JSON object per line, UTF-8, both ways. Glossa sends
 * {@code {"id": 7, "method": "translate", "params": {…}}}; the module answers
 * {@code {"id": 7, "result": …}} or {@code {"id": 7, "error": {…}}}. While
 * handling a request the module may send host calls
 * {@code {"id": "h1", "method": "http", "params": {…}}} and read Glossa's
 * {@code {"id": "h1", "result": …}} before answering.
 *
 * <p>{@link #run} points {@code System.out} at stderr, so a stray
 * {@code System.out.println} ends up in Glossa's log instead of the protocol.
 *
 * <p>Java 17 or newer; no dependencies.
 */
public final class Glossa {
    public static final int PROTOCOL_VERSION = 1;
    public static final String VERSION = "1.0.0";

    private Glossa() {}

    /** Serve requests on stdin/stdout until Glossa closes stdin, then exit. */
    public static void run(GlossaModule module) {
        OutputStream protocolOut = new FileOutputStream(FileDescriptor.out);
        System.setOut(System.err);
        try {
            serve(module, System.in, protocolOut);
        } catch (IOException e) {
            logLocal("the protocol stream failed: " + e);
        }
        System.exit(0);
    }

    /**
     * Serve requests from {@code in}, answering on {@code out}, until
     * {@code in} ends. {@link #run} uses it on the standard streams; tests
     * can drive a module through pipes.
     */
    public static void serve(GlossaModule module, InputStream in, OutputStream out) throws IOException {
        Channel channel = new Channel(in, out);
        while (true) {
            Map<String, Object> frame = channel.receive();
            if (frame == null) {
                return;
            }
            if (!frame.containsKey("id") || !(frame.get("method") instanceof String)) {
                continue;
            }
            serveOne(module, channel, frame);
        }
    }

    private static void serveOne(GlossaModule module, Channel channel, Map<String, Object> frame) throws IOException {
        Object id = frame.get("id");
        String method = (String) frame.get("method");
        Request req = new Request(method, Json.asMap(frame.get("params")));
        Host host = new Host(channel, req);
        String reply;
        try {
            Object result = normalise(method, dispatch(module, req, host));
            reply = Json.write(Json.obj("id", id, "result", result));
        } catch (ModuleError e) {
            reply = Json.write(Json.obj("id", id, "error", e.toWire()));
        } catch (Throwable e) {
            // Never let one request kill the module. A result holding
            // something JSON cannot carry also lands here, from Json.write.
            reply = Json.write(Json.obj("id", id, "error", Json.obj("kind", "other", "message", describe(e))));
        }
        channel.send(reply);
    }

    private static Object dispatch(GlossaModule module, Request req, Host host) throws Exception {
        switch (req.method()) {
            case "translate": return module.translate(req, host);
            case "detect": return module.detect(req, host);
            case "dictionary": return module.dictionary(req, host);
            case "speak": return module.speak(req, host);
            case "recognize": return module.recognize(req, host);
            default: return module.handle(req, host);
        }
    }

    /** The shorthands the protocol accepts from SDKs, expanded. */
    private static Object normalise(String method, Object result) {
        if (result instanceof CharSequence) {
            String s = result.toString();
            switch (method) {
                case "translate":
                case "recognize":
                    return Json.obj("text", s);
                case "detect":
                    return Json.obj("lang", s, "confidence", 1.0);
                default:
                    return s;
            }
        }
        return result;
    }

    private static String describe(Throwable e) {
        StringWriter trace = new StringWriter();
        e.printStackTrace(new PrintWriter(trace));
        return e.getClass().getName() + ": " + e.getMessage() + "\n" + trace;
    }

    static void logLocal(String message) {
        System.err.println("[glossa-sdk] " + message);
        System.err.flush();
    }

    /** The protocol stream. */
    static final class Channel {
        private final BufferedReader in;
        private final OutputStream out;
        private long nextHostId;

        Channel(InputStream in, OutputStream out) {
            this.in = new BufferedReader(new InputStreamReader(in, StandardCharsets.UTF_8), 64 * 1024);
            this.out = out;
        }

        synchronized void send(String json) throws IOException {
            byte[] line = (json + "\n").getBytes(StandardCharsets.UTF_8);
            out.write(line);
            out.flush();
        }

        /** The next frame, or null at the end of input. Lines that are not JSON objects are skipped. */
        Map<String, Object> receive() throws IOException {
            while (true) {
                String line = in.readLine();
                if (line == null) {
                    return null;
                }
                line = line.trim();
                if (line.isEmpty()) {
                    continue;
                }
                Object v;
                try {
                    v = Json.parse(line);
                } catch (IllegalArgumentException e) {
                    logLocal("ignoring a line that is not JSON: " + e.getMessage());
                    continue;
                }
                if (v instanceof Map) {
                    return Json.asMap(v);
                }
            }
        }

        Object hostCall(String method, Object params) throws ModuleError {
            String callId = "h" + (++nextHostId);
            try {
                send(Json.write(Json.obj("id", callId, "method", method, "params", params)));
                while (true) {
                    Map<String, Object> frame = receive();
                    if (frame == null) {
                        throw new ModuleError("other", "Glossa closed the connection");
                    }
                    if (!callId.equals(frame.get("id"))) {
                        logLocal("ignoring an unexpected frame while waiting for " + callId);
                        continue;
                    }
                    Object err = frame.get("error");
                    if (err != null) {
                        throw ModuleError.fromWire(err);
                    }
                    return frame.get("result");
                }
            } catch (IOException e) {
                throw new ModuleError("other", "the protocol stream failed: " + e.getMessage());
            }
        }
    }
}
