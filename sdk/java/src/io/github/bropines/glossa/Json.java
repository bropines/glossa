package io.github.bropines.glossa;

import java.lang.reflect.Array;
import java.lang.reflect.RecordComponent;
import java.util.ArrayList;
import java.util.Base64;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;

/**
 * A small JSON reader and writer, enough for the module protocol and for
 * the web APIs a module talks to.
 *
 * <p>Parsed values are plain Java objects: {@code Map<String, Object>} (in
 * document order), {@code List<Object>}, {@code String}, {@code Long} for
 * integers that fit, {@code Double} for other numbers, {@code Boolean} and
 * {@code null}.
 *
 * <p>The writer takes those, plus any {@code Map}, {@code Iterable} or
 * array, other {@code Number}s, {@code CharSequence}, {@code Character},
 * enums (by name), {@code Optional}, {@code byte[]} (as base64, the
 * protocol's encoding for binary data) and records (as objects of their
 * components, so {@code record Result(String text, String detected_from)}
 * serialises the way the protocol expects).
 */
public final class Json {
    private Json() {}

    /** Parse one JSON value; anything after it but whitespace is an error. */
    public static Object parse(String text) {
        Parser p = new Parser(text);
        p.skipWhitespace();
        Object value = p.value(0);
        p.skipWhitespace();
        if (p.pos != text.length()) {
            throw p.error("unexpected text after the value");
        }
        return value;
    }

    /** Serialise a value compactly, non-ASCII characters as they are. */
    public static String write(Object value) {
        StringBuilder out = new StringBuilder();
        write(value, out, 0);
        return out.toString();
    }

    /** An ordered map from alternating keys and values: {@code obj("text", t, "detected_from", "en")}. */
    public static Map<String, Object> obj(Object... keysAndValues) {
        if (keysAndValues.length % 2 != 0) {
            throw new IllegalArgumentException("obj() takes key, value pairs");
        }
        Map<String, Object> map = new LinkedHashMap<>();
        for (int i = 0; i < keysAndValues.length; i += 2) {
            map.put(String.valueOf(keysAndValues[i]), keysAndValues[i + 1]);
        }
        return map;
    }

    /** A parsed object, or an empty map when the value is not one. */
    @SuppressWarnings("unchecked")
    public static Map<String, Object> asMap(Object value) {
        return value instanceof Map ? (Map<String, Object>) value : new LinkedHashMap<>();
    }

    /** A parsed array, or an empty list when the value is not one. */
    @SuppressWarnings("unchecked")
    public static List<Object> asList(Object value) {
        return value instanceof List ? (List<Object>) value : new ArrayList<>();
    }

    /** A string field of an object, or {@code fallback}. */
    public static String string(Object object, String key, String fallback) {
        Object v = asMap(object).get(key);
        return v instanceof String ? (String) v : fallback;
    }

    /** Thrown for malformed JSON, with the offset of the problem. */
    public static final class ParseException extends IllegalArgumentException {
        private static final long serialVersionUID = 1L;
        public final int offset;

        ParseException(String message, int offset) {
            super(message + " at offset " + offset);
            this.offset = offset;
        }
    }

    // -- writing -------------------------------------------------------------

    private static final int MAX_DEPTH = 512;

    private static void write(Object v, StringBuilder out, int depth) {
        if (depth > MAX_DEPTH) {
            throw new IllegalArgumentException("value nested too deeply (a cycle?)");
        }
        if (v == null) {
            out.append("null");
        } else if (v instanceof CharSequence || v instanceof Character) {
            string(v.toString(), out);
        } else if (v instanceof Boolean) {
            out.append(v);
        } else if (v instanceof Double || v instanceof Float) {
            double d = ((Number) v).doubleValue();
            if (Double.isNaN(d) || Double.isInfinite(d)) {
                out.append("null");
            } else if (d == Math.rint(d) && Math.abs(d) < 1e15) {
                out.append((long) d);
            } else {
                out.append(d);
            }
        } else if (v instanceof Number) {
            out.append(v);
        } else if (v instanceof Map) {
            out.append('{');
            boolean first = true;
            for (Map.Entry<?, ?> e : ((Map<?, ?>) v).entrySet()) {
                if (!first) {
                    out.append(',');
                }
                first = false;
                string(String.valueOf(e.getKey()), out);
                out.append(':');
                write(e.getValue(), out, depth + 1);
            }
            out.append('}');
        } else if (v instanceof Iterable) {
            out.append('[');
            boolean first = true;
            for (Object item : (Iterable<?>) v) {
                if (!first) {
                    out.append(',');
                }
                first = false;
                write(item, out, depth + 1);
            }
            out.append(']');
        } else if (v instanceof byte[]) {
            string(Base64.getEncoder().encodeToString((byte[]) v), out);
        } else if (v.getClass().isArray()) {
            out.append('[');
            int n = Array.getLength(v);
            for (int i = 0; i < n; i++) {
                if (i > 0) {
                    out.append(',');
                }
                write(Array.get(v, i), out, depth + 1);
            }
            out.append(']');
        } else if (v instanceof Optional) {
            write(((Optional<?>) v).orElse(null), out, depth);
        } else if (v instanceof Enum) {
            string(((Enum<?>) v).name(), out);
        } else if (v instanceof Record) {
            Map<String, Object> fields = new LinkedHashMap<>();
            for (RecordComponent c : v.getClass().getRecordComponents()) {
                try {
                    c.getAccessor().setAccessible(true);
                    fields.put(c.getName(), c.getAccessor().invoke(v));
                } catch (ReflectiveOperationException | RuntimeException e) {
                    throw new IllegalArgumentException("cannot read " + c.getName() + " of " + v.getClass(), e);
                }
            }
            write(fields, out, depth);
        } else {
            throw new IllegalArgumentException("cannot write a " + v.getClass().getName() + " as JSON");
        }
    }

    private static void string(String s, StringBuilder out) {
        out.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"': out.append("\\\""); break;
                case '\\': out.append("\\\\"); break;
                case '\n': out.append("\\n"); break;
                case '\r': out.append("\\r"); break;
                case '\t': out.append("\\t"); break;
                case '\b': out.append("\\b"); break;
                case '\f': out.append("\\f"); break;
                default:
                    // U+2028/U+2029 are valid JSON but break line-based
                    // readers in some languages; escaping them costs nothing.
                    if (c < 0x20 || c == ' ' || c == ' ') {
                        out.append(String.format("\\u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
            }
        }
        out.append('"');
    }

    // -- reading -------------------------------------------------------------

    private static final class Parser {
        final String s;
        int pos;

        Parser(String s) {
            this.s = s;
        }

        ParseException error(String message) {
            return new ParseException(message, pos);
        }

        void skipWhitespace() {
            while (pos < s.length()) {
                char c = s.charAt(pos);
                if (c == ' ' || c == '\t' || c == '\n' || c == '\r') {
                    pos++;
                } else {
                    break;
                }
            }
        }

        Object value(int depth) {
            if (depth > MAX_DEPTH) {
                throw error("nested too deeply");
            }
            if (pos >= s.length()) {
                throw error("unexpected end of input");
            }
            char c = s.charAt(pos);
            switch (c) {
                case '{': return object(depth);
                case '[': return array(depth);
                case '"': return string();
                case 't': return literal("true", Boolean.TRUE);
                case 'f': return literal("false", Boolean.FALSE);
                case 'n': return literal("null", null);
                default:
                    if (c == '-' || (c >= '0' && c <= '9')) {
                        return number();
                    }
                    throw error("unexpected character '" + c + "'");
            }
        }

        Object literal(String word, Object value) {
            if (!s.startsWith(word, pos)) {
                throw error("expected " + word);
            }
            pos += word.length();
            return value;
        }

        Map<String, Object> object(int depth) {
            Map<String, Object> map = new LinkedHashMap<>();
            pos++; // {
            skipWhitespace();
            if (peek() == '}') {
                pos++;
                return map;
            }
            while (true) {
                skipWhitespace();
                if (peek() != '"') {
                    throw error("expected a string key");
                }
                String key = string();
                skipWhitespace();
                expect(':');
                skipWhitespace();
                map.put(key, value(depth + 1));
                skipWhitespace();
                char c = next();
                if (c == '}') {
                    return map;
                }
                if (c != ',') {
                    pos--;
                    throw error("expected ',' or '}'");
                }
            }
        }

        List<Object> array(int depth) {
            List<Object> list = new ArrayList<>();
            pos++; // [
            skipWhitespace();
            if (peek() == ']') {
                pos++;
                return list;
            }
            while (true) {
                skipWhitespace();
                list.add(value(depth + 1));
                skipWhitespace();
                char c = next();
                if (c == ']') {
                    return list;
                }
                if (c != ',') {
                    pos--;
                    throw error("expected ',' or ']'");
                }
            }
        }

        String string() {
            pos++; // opening quote
            StringBuilder out = null;
            int start = pos;
            while (true) {
                if (pos >= s.length()) {
                    throw error("unterminated string");
                }
                char c = s.charAt(pos);
                if (c == '"') {
                    String tail = s.substring(start, pos);
                    pos++;
                    return out == null ? tail : out.append(tail).toString();
                }
                if (c < 0x20) {
                    throw error("control character in a string");
                }
                if (c != '\\') {
                    pos++;
                    continue;
                }
                if (out == null) {
                    out = new StringBuilder();
                }
                out.append(s, start, pos);
                pos++; // backslash
                char e = next();
                switch (e) {
                    case '"': out.append('"'); break;
                    case '\\': out.append('\\'); break;
                    case '/': out.append('/'); break;
                    case 'b': out.append('\b'); break;
                    case 'f': out.append('\f'); break;
                    case 'n': out.append('\n'); break;
                    case 'r': out.append('\r'); break;
                    case 't': out.append('\t'); break;
                    case 'u':
                        if (pos + 4 > s.length()) {
                            throw error("short \\u escape");
                        }
                        try {
                            out.append((char) Integer.parseInt(s.substring(pos, pos + 4), 16));
                        } catch (NumberFormatException ex) {
                            throw error("bad \\u escape");
                        }
                        pos += 4; // surrogate pairs come out as two chars, as Java strings hold them
                        break;
                    default:
                        pos--;
                        throw error("bad escape \\" + e);
                }
                start = pos;
            }
        }

        Object number() {
            int start = pos;
            if (peek() == '-') {
                pos++;
            }
            if (peek() == '0') {
                pos++;
            } else if (isDigit(peek())) {
                while (isDigit(peek())) {
                    pos++;
                }
            } else {
                throw error("bad number");
            }
            boolean integral = true;
            if (peek() == '.') {
                integral = false;
                pos++;
                if (!isDigit(peek())) {
                    throw error("bad number");
                }
                while (isDigit(peek())) {
                    pos++;
                }
            }
            if (peek() == 'e' || peek() == 'E') {
                integral = false;
                pos++;
                if (peek() == '+' || peek() == '-') {
                    pos++;
                }
                if (!isDigit(peek())) {
                    throw error("bad number");
                }
                while (isDigit(peek())) {
                    pos++;
                }
            }
            String text = s.substring(start, pos);
            if (integral) {
                try {
                    return Long.parseLong(text);
                } catch (NumberFormatException tooBig) {
                    // falls through to a double
                }
            }
            return Double.parseDouble(text);
        }

        static boolean isDigit(char c) {
            return c >= '0' && c <= '9';
        }

        char peek() {
            return pos < s.length() ? s.charAt(pos) : '\0';
        }

        char next() {
            if (pos >= s.length()) {
                throw error("unexpected end of input");
            }
            return s.charAt(pos++);
        }

        void expect(char c) {
            if (next() != c) {
                pos--;
                throw error("expected '" + c + "'");
            }
        }
    }
}
