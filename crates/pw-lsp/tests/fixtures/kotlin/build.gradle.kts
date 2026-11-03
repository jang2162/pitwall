// Kotlin Gradle 플러그인(플러그인 포털)과 kotlin-stdlib(Maven Central), 그 전이 의존을 받는다 — 판 고정, 받은 것의 sha256 은
// gradle/verification-metadata.xml 이 고정한다(맞지 않으면 Gradle 이 빌드를 멈춘다).
plugins {
    kotlin("jvm") version "2.4.21"
}

repositories {
    mavenCentral()
}
