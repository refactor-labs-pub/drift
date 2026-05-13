plugins {
    kotlin("jvm") version "1.9.22"
    application
}

application {
    mainClass.set("com.example.MainKt")
}

repositories { mavenCentral() }

dependencies {
    implementation("io.ktor:ktor-server-core:2.3.7")
    implementation("io.ktor:ktor-server-netty:2.3.7")
}
