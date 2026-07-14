using System;
using System.Reflection;
using System.IO;

namespace s3b
{
    public class BackupLog : Model
    {
        public BackupLog()
        {
            tableName = "backup_log";
        }

        public static BackupLog factory( Config args, LocalFolder parent, FileInfo fi)
        {
            BackupLog b = new BackupLog();

            b.backup_time = DateTime.Now;
            b.last_upload_time = parent.upload_datetime;
            b.last_write_time = fi.LastWriteTime;
            b.hostname = System.Environment.MachineName;
            b.username = System.Environment.UserName;
            b.bucket = Config.getConfig().getString("bucket");
            b.parent_folder = parent.folder_path;
            b.file_path = fi.FullName;

            return b;
        }

        public long id
        {
            get
            {
                return Convert.ToInt64(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }
        public string hostname
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string username
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string bucket
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string parent_folder
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public string file_path
        {
            get
            {
                return getPropValue(MethodBase.GetCurrentMethod().Name).ToString();
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public DateTime backup_time
        {
            get
            {
                return Convert.ToDateTime(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public DateTime last_write_time
        {
            get
            {
                return Convert.ToDateTime(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }

        public DateTime last_upload_time
        {
            get
            {
                return Convert.ToDateTime(getPropValue(MethodBase.GetCurrentMethod().Name));
            }
            set
            {
                setPropValue(MethodBase.GetCurrentMethod().Name, value);
            }
        }
    }
}

