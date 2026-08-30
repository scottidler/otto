# define vars
CERTS_DIR	?= ./certs/example

.DEFAULT_GOAL := dev

.PHONY: all
all: dev unit-test integration-test type-check lint

# Dev
.PHONY: dev
dev:
	poetry install


.PHONY: unit-test
unit-test: dev
	env ENV=ci poetry run pytest --junitxml=pytest.xml --cov=example_service tests/unit

.PHONY: integration-test
integration-test: dev
	poetry run pytest tests/integration

.PHONY: type-check
type-check: dev
	poetry run mypy

.PHONY: lint
lint: dev
	poetry run pre-commit run --all-files

# Clean
.PHONY: clean
clean:
	rm -rf build/ dist/ .venv/ .mypy_cache/ .pytest_cache/
	rm -rf *.egg-info
	rm -rf .coverage
	find . -name '__pycache__' -delete
	find . -name '*.pyc' -delete

# key generation
.PHONY: keygen
keygen:
	# Create separate directories for public and private keys
	mkdir -p $(CERTS_DIR)/public $(CERTS_DIR)/private

	# gen active.pem
	# generate active.pem private key and extract active.pem public key and put it in the public directory
	openssl genpkey -algorithm RSA -out $(CERTS_DIR)/private/active.pem -pkeyopt rsa_keygen_bits:2048
	openssl rsa -in $(CERTS_DIR)/private/active.pem -pubout -out $(CERTS_DIR)/public/active.pem

	# gen on_deck.pem
	# generate on_deck.pem private key and extract on_deck.pem public key and put it in the public directory
	openssl genpkey -algorithm RSA -out $(CERTS_DIR)/private/on_deck.pem -pkeyopt rsa_keygen_bits:2048
	openssl rsa -in $(CERTS_DIR)/private/on_deck.pem -pubout -out $(CERTS_DIR)/public/on_deck.pem

	@echo "Private keys stored in $(CERTS_DIR)/private/$(CLIENT_ID).pem"
	@echo "Public keys stored in $(CERTS_DIR)/public/$(CLIENT_ID).pem"
