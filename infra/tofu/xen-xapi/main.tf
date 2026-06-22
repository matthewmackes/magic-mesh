# Read-only connectivity proof: list hosts + SRs straight off XAPI. No resources,
# no mutations — this only proves the XAPI-native provider authenticates and reads
# the live pool, the first gate of the DATACENTER-1 migration assessment.
data "xenserver_host" "all" {}
data "xenserver_sr" "all" {}
