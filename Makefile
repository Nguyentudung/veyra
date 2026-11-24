PYTHON ?= python3
RUNS ?= 3

.PHONY: test benchmark benchmark-headless

test:
	cargo test --workspace
	$(PYTHON) -B -m unittest discover -s benchmarks -p 'test_*.py' -v

benchmark:
	$(PYTHON) -B benchmarks/benchmark.py --build-veyra --runs $(RUNS)

# Xvfb does not unset a parent Wayland socket. Force Rune/winit to X11 here
# while leaving the ordinary desktop benchmark on its native backend.
benchmark-headless:
	xvfb-run -a env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET WINIT_UNIX_BACKEND=x11 $(PYTHON) -B benchmarks/benchmark.py --build-veyra --headless --runs $(RUNS)
