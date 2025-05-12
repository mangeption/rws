# Build and run targets for WebSocket server with Autobahn tests

# Run the server and tests
run-server:
	cargo run --release & SERVER_PID=$$!; \
	docker run \
		--name fuzzingserver \
		-v ${PWD}/config/fuzzingclient.json:/fuzzingclient.json \
		-v ${PWD}/reports:/reports \
		--net=host \
		--rm \
		crossbario/autobahn-testsuite:latest \
		wstest -m fuzzingclient -s fuzzingclient.json; \
	kill $$SERVER_PID

# Clean up any running containers
clean:
	docker rm -f fuzzingserver 2>/dev/null || true

