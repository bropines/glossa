package io.github.bropines.glossa;

import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;

/**
 * Marks a method of a {@link GlossaModule} as the handler of an
 * {@code [[actions]]} entry: the value is the entry's {@code handler}, or
 * its {@code id} when it names no handler.
 *
 * <pre>{@code
 * @Action("list_models")
 * public Object listModels(Request ev, Host host) throws Exception {
 *     return Json.obj("choices", Json.obj("model", List.of("small", "large")));
 * }
 * }</pre>
 *
 * <p>The method takes the request ({@link Request#action()},
 * {@link Request#state()}, the options) and a {@link Host}, and returns the
 * reply: a map with {@code options}, {@code choices}, {@code errors},
 * {@code notice}, {@code notice_level} (all optional), just the notice as a
 * String, or null.
 */
@Retention(RetentionPolicy.RUNTIME)
@Target(ElementType.METHOD)
public @interface Action {
    String value();
}
