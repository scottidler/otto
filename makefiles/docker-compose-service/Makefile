GIT_DESCRIBE	:= $(shell git describe --always --abbrev=7)
BUILD_OWNER	?= $(shell cat ./build.conf | grep build-url | awk -F '[//|@]' '{print $$3}' | awk -F '[:]' '{print $$1}')
BUILD_TAG	?= $(shell cat ./build.conf | grep build-url | awk -F '[//|@]' '{print $$3}' | awk -F '[:]' '{print $$2}')
.DEFAULT_GOAL 	:= dev

.PHONY: all
all: dev unit-test integration-test type-check lint

# Dev
.PHONY: dev
dev:
	poetry install


# Testing related targets
.PHONY: test
test: dev
	poetry run pytest

.PHONY: unit-test
unit-test: dev
	poetry run pytest --junitxml=pytest.xml --cov=example_service tests/unit --log-cli-level=CRITICAL

.PHONY: integration-test
integration-test: dev
	poetry run pytest tests/integration -s || true

type-check: dev
	poetry run mypy

lint: dev
	poetry run pre-commit run --all-files

run-rest: dev
	poetry run uvicorn example_service.rest.app:app --reload

# Clean
clean: dev-clean
	rm -rf build/ dist/ .venv/ .mypy_cache/ .pytest_cache/
	rm -rf *.egg-info
	rm -rf .coverage
	find . -name '*.pyc' -delete
	find . -name '__pycache__' -delete

# --
# -- Dev/local only
# --

LOCAL_DUMP_FILE:=./schema/local-db/example-service-pgdump.tar.gz
LATEST_S3_DUMP_FILE:=$(shell aws s3api list-objects-v2 --bucket "example-pg-dumps" --prefix 'example_service' --query 'sort_by(Contents, &LastModified)[-1].Key' --output=text)
DUMP_CHECK:=$(LOCAL_DUMP_FILE)-$(LATEST_S3_DUMP_FILE).check

dev-clean:
	rm -f $(LOCAL_DUMP_FILE)-*.check

get-postgres-dump:
	@echo "LATEST_S3_DUMP_FILE: $(LATEST_S3_DUMP_FILE)"
	@echo "Downloading the latest pgdump from S3.."
	@[ -f "$(DUMP_CHECK)" ] && echo "  dump file already exists" \
		|| aws s3 cp s3://example-pg-dumps/$(LATEST_S3_DUMP_FILE) $(LOCAL_DUMP_FILE)
	@touch "$(DUMP_CHECK)"

# -- normal Docker compose build, this is the preferred method for running this service locally

docker-compose-build:
	docker compose up --build -d

docker-compose: get-postgres-dump docker-compose-build

docker-compose-down:
	docker compose -f docker-compose.yaml down

docker-compose-with-sidecar-build:
	docker compose -f docker-compose.yaml -f docker-compose.sidecar.yaml up --build -d

docker-compose-with-sidecar: get-postgres-dump docker-compose-with-sidecar-build

docker-compose-with-sidecar-down:
	docker compose -f docker-compose.yaml -f docker-compose.sidecar.yaml down

docker-compose-clean:
	@echo "Deleting example-service volumes and containers..."
	docker ps -a | grep 'example-service'  | awk -F ' ' '{print $$1}' | xargs -I {} sh -c "docker rm -f {}"
	docker volume list | grep 'example-service' | awk -F ' ' '{print $$2}' | xargs -I {} sh -c "docker volume rm -f {}"

psql:
	docker exec -it example-service-db-1 psql -U example example_service
