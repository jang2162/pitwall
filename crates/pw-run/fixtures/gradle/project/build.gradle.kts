tasks.register("hello") {
    group = "custom"
    description = "Says hello"
    doLast { println("hello") }
}
