plugins {
    id("com.android.library") version "8.13.0"
    id("org.jetbrains.kotlin.android") version "2.2.20"
}

android {
    namespace = "org.autoseaway.mobile"
    compileSdk = 36
    defaultConfig {
        minSdk = 21
    }
    sourceSets["main"].kotlin.srcDir("../../../target/uniffi/kotlin")
    sourceSets["main"].jniLibs.srcDir("../../../target/jniLibs")
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation("net.java.dev.jna:jna:5.14.0@aar")
}
