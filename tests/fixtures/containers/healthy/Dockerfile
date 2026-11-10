FROM busybox:1.36
HEALTHCHECK --interval=1s --timeout=1s --retries=60 CMD test -f /tmp/healthy
CMD ["sh", "-c", "trap 'exit 0' TERM; echo getting ready; sleep 2; touch /tmp/healthy; echo healthy now; sleep 3600 & wait"]
