# Blank-slate Java image — installs ONLY the packaged snapdir.jar artifact.
# eclipse-temurin:17-jdk provides JDK 17 (incubator.foreign API) and javac.
# The jar embeds the native .so which NativeLoader extracts at runtime — zero
# build tools beyond the JDK needed.

FROM eclipse-temurin:17-jdk

WORKDIR /app

# Copy the packaged snapdir.jar (embeds the native .so; staged by run_relay.sh).
COPY snapdir.jar ./

# Copy and compile the canonical example app against the jar.
COPY App.java ./
RUN javac --release 17 --add-modules jdk.incubator.foreign -cp snapdir.jar App.java

ENTRYPOINT ["java", \
    "--add-modules", "jdk.incubator.foreign", \
    "--enable-native-access=ALL-UNNAMED", \
    "-cp", "snapdir.jar:.", \
    "App"]
