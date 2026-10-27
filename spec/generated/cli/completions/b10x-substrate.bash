_b10x__substrate() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="b10x__substrate"
                ;;
            b10x__substrate,completions)
                cmd="b10x__substrate__subcmd__completions"
                ;;
            b10x__substrate,confine)
                cmd="b10x__substrate__subcmd__confine"
                ;;
            b10x__substrate,events)
                cmd="b10x__substrate__subcmd__events"
                ;;
            b10x__substrate,exec)
                cmd="b10x__substrate__subcmd__exec"
                ;;
            b10x__substrate,help)
                cmd="b10x__substrate__subcmd__help"
                ;;
            b10x__substrate,machine)
                cmd="b10x__substrate__subcmd__machine"
                ;;
            b10x__substrate,metrics)
                cmd="b10x__substrate__subcmd__metrics"
                ;;
            b10x__substrate,operation)
                cmd="b10x__substrate__subcmd__operation"
                ;;
            b10x__substrate,session)
                cmd="b10x__substrate__subcmd__session"
                ;;
            b10x__substrate,workspace)
                cmd="b10x__substrate__subcmd__workspace"
                ;;
            b10x__substrate__subcmd__confine,help)
                cmd="b10x__substrate__subcmd__confine__subcmd__help"
                ;;
            b10x__substrate__subcmd__confine,run)
                cmd="b10x__substrate__subcmd__confine__subcmd__run"
                ;;
            b10x__substrate__subcmd__confine__subcmd__help,help)
                cmd="b10x__substrate__subcmd__confine__subcmd__help__subcmd__help"
                ;;
            b10x__substrate__subcmd__confine__subcmd__help,run)
                cmd="b10x__substrate__subcmd__confine__subcmd__help__subcmd__run"
                ;;
            b10x__substrate__subcmd__exec,help)
                cmd="b10x__substrate__subcmd__exec__subcmd__help"
                ;;
            b10x__substrate__subcmd__exec,run)
                cmd="b10x__substrate__subcmd__exec__subcmd__run"
                ;;
            b10x__substrate__subcmd__exec,show)
                cmd="b10x__substrate__subcmd__exec__subcmd__show"
                ;;
            b10x__substrate__subcmd__exec__subcmd__help,help)
                cmd="b10x__substrate__subcmd__exec__subcmd__help__subcmd__help"
                ;;
            b10x__substrate__subcmd__exec__subcmd__help,run)
                cmd="b10x__substrate__subcmd__exec__subcmd__help__subcmd__run"
                ;;
            b10x__substrate__subcmd__exec__subcmd__help,show)
                cmd="b10x__substrate__subcmd__exec__subcmd__help__subcmd__show"
                ;;
            b10x__substrate__subcmd__help,completions)
                cmd="b10x__substrate__subcmd__help__subcmd__completions"
                ;;
            b10x__substrate__subcmd__help,confine)
                cmd="b10x__substrate__subcmd__help__subcmd__confine"
                ;;
            b10x__substrate__subcmd__help,events)
                cmd="b10x__substrate__subcmd__help__subcmd__events"
                ;;
            b10x__substrate__subcmd__help,exec)
                cmd="b10x__substrate__subcmd__help__subcmd__exec"
                ;;
            b10x__substrate__subcmd__help,help)
                cmd="b10x__substrate__subcmd__help__subcmd__help"
                ;;
            b10x__substrate__subcmd__help,machine)
                cmd="b10x__substrate__subcmd__help__subcmd__machine"
                ;;
            b10x__substrate__subcmd__help,metrics)
                cmd="b10x__substrate__subcmd__help__subcmd__metrics"
                ;;
            b10x__substrate__subcmd__help,operation)
                cmd="b10x__substrate__subcmd__help__subcmd__operation"
                ;;
            b10x__substrate__subcmd__help,session)
                cmd="b10x__substrate__subcmd__help__subcmd__session"
                ;;
            b10x__substrate__subcmd__help,workspace)
                cmd="b10x__substrate__subcmd__help__subcmd__workspace"
                ;;
            b10x__substrate__subcmd__help__subcmd__confine,run)
                cmd="b10x__substrate__subcmd__help__subcmd__confine__subcmd__run"
                ;;
            b10x__substrate__subcmd__help__subcmd__exec,run)
                cmd="b10x__substrate__subcmd__help__subcmd__exec__subcmd__run"
                ;;
            b10x__substrate__subcmd__help__subcmd__exec,show)
                cmd="b10x__substrate__subcmd__help__subcmd__exec__subcmd__show"
                ;;
            b10x__substrate__subcmd__help__subcmd__operation,show)
                cmd="b10x__substrate__subcmd__help__subcmd__operation__subcmd__show"
                ;;
            b10x__substrate__subcmd__help__subcmd__session,show)
                cmd="b10x__substrate__subcmd__help__subcmd__session__subcmd__show"
                ;;
            b10x__substrate__subcmd__help__subcmd__session,signal)
                cmd="b10x__substrate__subcmd__help__subcmd__session__subcmd__signal"
                ;;
            b10x__substrate__subcmd__help__subcmd__workspace,create)
                cmd="b10x__substrate__subcmd__help__subcmd__workspace__subcmd__create"
                ;;
            b10x__substrate__subcmd__help__subcmd__workspace,destroy)
                cmd="b10x__substrate__subcmd__help__subcmd__workspace__subcmd__destroy"
                ;;
            b10x__substrate__subcmd__help__subcmd__workspace,renew)
                cmd="b10x__substrate__subcmd__help__subcmd__workspace__subcmd__renew"
                ;;
            b10x__substrate__subcmd__help__subcmd__workspace,show)
                cmd="b10x__substrate__subcmd__help__subcmd__workspace__subcmd__show"
                ;;
            b10x__substrate__subcmd__operation,help)
                cmd="b10x__substrate__subcmd__operation__subcmd__help"
                ;;
            b10x__substrate__subcmd__operation,show)
                cmd="b10x__substrate__subcmd__operation__subcmd__show"
                ;;
            b10x__substrate__subcmd__operation__subcmd__help,help)
                cmd="b10x__substrate__subcmd__operation__subcmd__help__subcmd__help"
                ;;
            b10x__substrate__subcmd__operation__subcmd__help,show)
                cmd="b10x__substrate__subcmd__operation__subcmd__help__subcmd__show"
                ;;
            b10x__substrate__subcmd__session,help)
                cmd="b10x__substrate__subcmd__session__subcmd__help"
                ;;
            b10x__substrate__subcmd__session,show)
                cmd="b10x__substrate__subcmd__session__subcmd__show"
                ;;
            b10x__substrate__subcmd__session,signal)
                cmd="b10x__substrate__subcmd__session__subcmd__signal"
                ;;
            b10x__substrate__subcmd__session__subcmd__help,help)
                cmd="b10x__substrate__subcmd__session__subcmd__help__subcmd__help"
                ;;
            b10x__substrate__subcmd__session__subcmd__help,show)
                cmd="b10x__substrate__subcmd__session__subcmd__help__subcmd__show"
                ;;
            b10x__substrate__subcmd__session__subcmd__help,signal)
                cmd="b10x__substrate__subcmd__session__subcmd__help__subcmd__signal"
                ;;
            b10x__substrate__subcmd__workspace,create)
                cmd="b10x__substrate__subcmd__workspace__subcmd__create"
                ;;
            b10x__substrate__subcmd__workspace,destroy)
                cmd="b10x__substrate__subcmd__workspace__subcmd__destroy"
                ;;
            b10x__substrate__subcmd__workspace,help)
                cmd="b10x__substrate__subcmd__workspace__subcmd__help"
                ;;
            b10x__substrate__subcmd__workspace,renew)
                cmd="b10x__substrate__subcmd__workspace__subcmd__renew"
                ;;
            b10x__substrate__subcmd__workspace,show)
                cmd="b10x__substrate__subcmd__workspace__subcmd__show"
                ;;
            b10x__substrate__subcmd__workspace__subcmd__help,create)
                cmd="b10x__substrate__subcmd__workspace__subcmd__help__subcmd__create"
                ;;
            b10x__substrate__subcmd__workspace__subcmd__help,destroy)
                cmd="b10x__substrate__subcmd__workspace__subcmd__help__subcmd__destroy"
                ;;
            b10x__substrate__subcmd__workspace__subcmd__help,help)
                cmd="b10x__substrate__subcmd__workspace__subcmd__help__subcmd__help"
                ;;
            b10x__substrate__subcmd__workspace__subcmd__help,renew)
                cmd="b10x__substrate__subcmd__workspace__subcmd__help__subcmd__renew"
                ;;
            b10x__substrate__subcmd__workspace__subcmd__help,show)
                cmd="b10x__substrate__subcmd__workspace__subcmd__help__subcmd__show"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        b10x__substrate)
            opts="-h --socket --output --help confine events exec machine metrics operation session workspace completions help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__completions)
            opts="-h --socket --output --help bash"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__confine)
            opts="-h --socket --output --help run help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__confine__subcmd__help)
            opts="run help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__confine__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__confine__subcmd__help__subcmd__run)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__confine__subcmd__run)
            opts="-h --dir --ro --net --backend --image --dry-run --local --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --ro)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --net)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --backend)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --image)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --dry-run)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --local)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__events)
            opts="-h --cursor --limit --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --cursor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --limit)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__exec)
            opts="-h --socket --output --help run show help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__exec__subcmd__help)
            opts="run show help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__exec__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__exec__subcmd__help__subcmd__run)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__exec__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__exec__subcmd__run)
            opts="-h --workspace --timeout-ms --cpu-millis --memory-bytes --processes --output-bytes --operation-id --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --workspace)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --timeout-ms)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --cpu-millis)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --memory-bytes)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --processes)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output-bytes)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --operation-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__exec__subcmd__show)
            opts="-h --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help)
            opts="confine events exec machine metrics operation session workspace completions help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__completions)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__confine)
            opts="run"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__confine__subcmd__run)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__events)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__exec)
            opts="run show"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__exec__subcmd__run)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__exec__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__machine)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__metrics)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__operation)
            opts="show"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__operation__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__session)
            opts="show signal"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__session__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__session__subcmd__signal)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__workspace)
            opts="create destroy renew show"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__workspace__subcmd__create)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__workspace__subcmd__destroy)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__workspace__subcmd__renew)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__help__subcmd__workspace__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__machine)
            opts="-h --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__metrics)
            opts="-h --kind --id --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --kind)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__operation)
            opts="-h --socket --output --help show help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__operation__subcmd__help)
            opts="show help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__operation__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__operation__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__operation__subcmd__show)
            opts="-h --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__session)
            opts="-h --socket --output --help show signal help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__session__subcmd__help)
            opts="show signal help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__session__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__session__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__session__subcmd__help__subcmd__signal)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__session__subcmd__show)
            opts="-h --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__session__subcmd__signal)
            opts="-h --signal --grace-ms --operation-id --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --signal)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --grace-ms)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --operation-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace)
            opts="-h --socket --output --help create destroy renew show help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace__subcmd__create)
            opts="-h --labels --storage --lease-ttl-ms --operation-id --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --labels)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --storage)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lease-ttl-ms)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --operation-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace__subcmd__destroy)
            opts="-h --operation-id --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --operation-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace__subcmd__help)
            opts="create destroy renew show help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace__subcmd__help__subcmd__create)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace__subcmd__help__subcmd__destroy)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace__subcmd__help__subcmd__renew)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace__subcmd__help__subcmd__show)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace__subcmd__renew)
            opts="-h --ttl-ms --operation-id --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --ttl-ms)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --operation-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        b10x__subcmd__substrate__subcmd__workspace__subcmd__show)
            opts="-h --socket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --socket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "human json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _b10x__substrate -o nosort -o bashdefault -o default b10x-substrate
else
    complete -F _b10x__substrate -o bashdefault -o default b10x-substrate
fi
