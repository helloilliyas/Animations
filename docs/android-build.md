# Building and installing the Android app

The app is a [Tauri 2](https://v2.tauri.app/) shell (`app/src-tauri`) around
the Rust service, with the UI in `app/ui`. These steps build a signed APK you
can install on your own phone.

## 1. One-time setup (Linux, macOS or Windows/WSL)

1. **Rust**: install from <https://rustup.rs>, then add the Android targets:
   ```sh
   rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
   ```
2. **Java 17 or newer** (e.g. Temurin, or the JDK bundled with Android Studio).
3. **Android SDK + NDK**: easiest via Android Studio → SDK Manager. Install
   *Android SDK Platform 36*, *Build-Tools 36.x*, *Platform-Tools* and
   *NDK (Side by side)* 28.x. Then set:
   ```sh
   export ANDROID_HOME=$HOME/Android/Sdk            # where the SDK lives
   export NDK_HOME=$ANDROID_HOME/ndk/28.2.13676358  # your NDK version
   export JAVA_HOME=/path/to/jdk
   ```
4. **Tauri CLI**:
   ```sh
   cargo install tauri-cli --version "^2" --locked
   ```

## 2. Generate the Android project (first time only)

```sh
cd app/src-tauri
cargo tauri icon app-icon.png   # app icons from the 1024px source
cargo tauri android init        # creates gen/android
```

## 3. Build

```sh
cd app/src-tauri
cargo tauri android build --apk --target aarch64
```

Most phones from the last several years are `aarch64` (arm64). The unsigned APK
is written to:

```
gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk
```

If Gradle fails with **429 Too Many Requests** from Maven Central (common on
shared networks), point it at Google's mirror of Maven Central by saving this
as `~/.gradle/init.d/maven-mirror.gradle` and building again:

```groovy
def mirror = 'https://maven-central.storage-download.googleapis.com/maven2/'
beforeSettings { settings ->
    settings.pluginManagement.repositories {
        maven { url mirror; name 'CentralMirror' }
        google()
        gradlePluginPortal()
    }
}
allprojects {
    buildscript.repositories { maven { url mirror; name 'CentralMirror' } }
    repositories { maven { url mirror; name 'CentralMirror' } }
}
```

## 4. Sign

Android only installs signed APKs. Create a key once and **keep it safe**:
updates must be signed with the same key, or you will have to uninstall first
(which deletes the app's data).

```sh
keytool -genkeypair -v -keystore ~/cse-trader.jks -alias cse-trader \
  -keyalg RSA -keysize 2048 -validity 10000

BT=$ANDROID_HOME/build-tools/36.1.0
APK=gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk
$BT/zipalign -f -p 4 "$APK" cse-trader-aligned.apk
$BT/apksigner sign --ks ~/cse-trader.jks --ks-key-alias cse-trader \
  --out cse-trader.apk cse-trader-aligned.apk
$BT/apksigner verify cse-trader.apk
```

## 5. Install on the phone

- **USB**: enable *Developer options → USB debugging*, connect, then
  `adb install -r cse-trader.apk`.
- **Without a computer**: copy `cse-trader.apk` to the phone (Google Drive,
  email, USB), open it in Files, and allow *Install unknown apps* for that app
  when Android asks.

## Trying it in a desktop browser

No Android tools needed:

```sh
cargo run -p cse-dev-server
# open http://127.0.0.1:8787 (use the browser's phone-size view)
```

The dev server serves `app/ui` and the same command API the app uses, with its
own database file (`cse-trader-dev.sqlite`).
