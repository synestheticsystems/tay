FROM busybox:1.36
RUN rm /bin/chown
CMD ["sh", "-c", "trap 'exit 0' TERM; sleep 3600 & wait"]
