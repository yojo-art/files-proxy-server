set -eu
docker build . -t files-agent
docker run -it --rm --name files-agent -v ./:/config -e FILES_AGENT_CONFIG_PATH=/config/config.json files-agent
