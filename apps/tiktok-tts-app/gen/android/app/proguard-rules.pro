# Preserve all classes, interfaces, methods, and fields in com.sanichka.yapper package
# Critical for JNI interaction between Rust cdylib and Kotlin
-keep class com.sanichka.yapper.** { *; }
-keepclassmembers class com.sanichka.yapper.** { *; }

# Keep native methods and classes containing them
-keepclasseswithmembernames class * {
    native <methods>;
}

# Preserve any classes or members marked with @Keep
-keep class androidx.annotation.Keep
-keep @androidx.annotation.Keep class * { *; }
-keepclassmembers class * {
    @androidx.annotation.Keep *;
}
-keepclasseswithmembers class * {
    @androidx.annotation.Keep <methods>;
}
-keepclasseswithmembers class * {
    @androidx.annotation.Keep <fields>;
}

# Preserve MediaStyle and AndroidX media session classes
-keep class androidx.media.** { *; }
-keep class android.support.v4.media.** { *; }